//! Expression emission: `RK(C)`/`sC` selection, arithmetic/comparison codegen, and peephole.

use crate::compiler::fold::{fold_binop_value, fold_unop_value};
use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::proto::NO_JUMP;
use crate::compiler::scope::{CompileError, ExprDesc, ExprKind, FunctionScope};
use crate::gc::GcAllocator;
use crate::value::Value;

/// Binary operators eligible for compile-time folding and specialized emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Band,
    Bor,
    Bxor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Unary operators eligible for compile-time folding and specialized emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    BNot,
    Not,
}

/// Encodes constant pool index `k` as an `RK(C)` operand (`C = 0x80 | k`) when `k <= 127`.
#[inline]
#[must_use]
pub const fn encode_rk(const_idx: u16) -> Option<u8> {
    if const_idx <= 127 {
        Some(0x80 | (const_idx as u8))
    } else {
        None
    }
}

/// Returns `Some(sC)` when `imm` fits in a signed 8-bit immediate (`-128..=127`).
#[inline]
#[must_use]
pub const fn encode_sc(imm: i64) -> Option<i8> {
    if imm >= -128 && imm <= 127 {
        Some(imm as i8)
    } else {
        None
    }
}

impl<'a> FunctionScope<'a> {
    /// Returns the next instruction PC (`proto.code_count`).
    #[inline]
    #[must_use]
    pub fn pc(&self) -> u32 {
        // SAFETY: `self.proto` is a valid `ObjProto` owned by the compiler.
        unsafe { (*self.proto).code_count }
    }

    /// Emits a raw instruction and returns its PC.
    pub fn emit(
        &mut self,
        alloc: &mut GcAllocator,
        inst: Instruction,
        line: u32,
    ) -> Result<u32, CompileError> {
        // SAFETY: `self.proto` is valid.
        unsafe {
            (*self.proto)
                .emit_instruction(alloc, inst, line)
                .map_err(|e| CompileError::new(e, line))
        }
    }

    /// Emits `OP_MOVE R[dst], R[src]`, suppressing the no-op `OP_MOVE R[A], R[A]`.
    pub fn emit_move(
        &mut self,
        alloc: &mut GcAllocator,
        dst: u8,
        src: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        if dst != src {
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::Move, dst, src, 0),
                line,
            )?;
        }
        Ok(())
    }

    /// Peephole: applying `not` to `ExprKind::Jump` flips the polarity bit of the comparison at
    /// `pc - 1` and swaps `patch_true` / `patch_false` without emitting extra instructions.
    pub fn invert_condition(&mut self, e: &mut ExprDesc) {
        if let ExprKind::Jump { pc } = e.kind {
            if pc > 0 {
                // SAFETY: `pc - 1` is the fused comparison/test preceding the trailing `OP_JMP`.
                unsafe {
                    let inst = &mut *(*self.proto).code.add((pc - 1) as usize);
                    let a = inst.a();
                    inst.patch_a(if a == 0 { 1 } else { 0 });
                }
            }
            core::mem::swap(&mut e.patch_true, &mut e.patch_false);
        }
    }

    /// Compiles unary `op e`, folding constants when possible and applying peephole `not` inversion.
    pub fn compile_unop(
        &mut self,
        alloc: &mut GcAllocator,
        op: UnOp,
        mut e: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        if op == UnOp::Not {
            if matches!(e.kind, ExprKind::Jump { .. }) {
                self.invert_condition(&mut e);
                return Ok(e);
            }
            if let Some(v) = self.expr_to_const_value(e.kind) {
                let folded = if v.is_falsy() {
                    ExprKind::True
                } else {
                    ExprKind::False
                };
                return Ok(ExprDesc::new(folded));
            }
        } else if let Some(v) = self.expr_to_const_value(e.kind)
            && let Some(folded) = fold_unop_value(op, v)
        {
            return Ok(ExprDesc::new(self.value_to_expr_kind(alloc, folded, line)?));
        }

        let reg = self.expr_to_any_reg(alloc, e, line)?;
        let dst = self.alloc_regs(1, line)?;
        let opcode = match op {
            UnOp::Neg => OpCode::Unm,
            UnOp::BNot => OpCode::BNot,
            UnOp::Not => OpCode::Not,
        };
        self.emit(alloc, Instruction::make_abc(opcode, dst, reg, 0), line)?;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    /// Compiles binary `left op right`, folding constants and selecting `RK(C)` / `sC` / `*I` forms.
    pub fn compile_binop(
        &mut self,
        alloc: &mut GcAllocator,
        op: BinOp,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        if let (Some(lv), Some(rv)) = (
            self.expr_to_const_value(left.kind),
            self.expr_to_const_value(right.kind),
        ) && let Some(folded) = fold_binop_value(op, lv, rv)
        {
            return Ok(ExprDesc::new(self.value_to_expr_kind(alloc, folded, line)?));
        }

        match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.emit_comparison(alloc, op, left, right, line)
            }
            _ => self.emit_arith(alloc, op, left, right, line),
        }
    }

    /// Materializes `e` into any register (reusing locals) and returns that register index.
    pub fn expr_to_any_reg(
        &mut self,
        alloc: &mut GcAllocator,
        e: ExprDesc,
        line: u32,
    ) -> Result<u8, CompileError> {
        match e.kind {
            ExprKind::Local { reg, .. } | ExprKind::NonReloc { reg } => Ok(reg),
            _ => {
                let reg = self.alloc_regs(1, line)?;
                self.discharge_to_reg(alloc, e, reg, line)?;
                Ok(reg)
            }
        }
    }

    /// Materializes `e` into `dst`, emitting load/move/getupval as needed.
    pub fn discharge_to_reg(
        &mut self,
        alloc: &mut GcAllocator,
        e: ExprDesc,
        dst: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        match e.kind {
            ExprKind::Void => {}
            ExprKind::Nil => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::LoadNil, dst, 1, 0),
                    line,
                )?;
            }
            ExprKind::True => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::LoadBool, dst, 1, 0),
                    line,
                )?;
            }
            ExprKind::False => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::LoadBool, dst, 0, 0),
                    line,
                )?;
            }
            ExprKind::IntImm(i) => {
                self.emit(
                    alloc,
                    Instruction::make_asbx(OpCode::LoadI, dst, i32::from(i)),
                    line,
                )?;
            }
            ExprKind::Const(k) => {
                self.emit(alloc, Instruction::make_abx(OpCode::LoadK, dst, k), line)?;
            }
            ExprKind::Local { reg, .. } | ExprKind::NonReloc { reg } => {
                self.emit_move(alloc, dst, reg, line)?;
            }
            ExprKind::Upvalue { idx, .. } => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::GetUpval, dst, idx, 0),
                    line,
                )?;
            }
            ExprKind::Indexed { table_reg, key_reg } => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::GetTab, dst, table_reg, key_reg),
                    line,
                )?;
            }
            ExprKind::Field { table_reg, key_k } => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::GetField, dst, table_reg, key_k),
                    line,
                )?;
            }
            ExprKind::Relocable { pc } => {
                // SAFETY: `pc` indexes a relocable instruction whose A is unpatched.
                unsafe {
                    (*(*self.proto).code.add(pc as usize)).patch_a(dst);
                }
            }
            ExprKind::Jump { .. } => {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::LoadBool, dst, 0, 0),
                    line,
                )?;
            }
        }
        Ok(())
    }

    fn emit_arith(
        &mut self,
        alloc: &mut GcAllocator,
        op: BinOp,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        if op == BinOp::Add
            && let Some(sc) = imm_sc(right.kind)
        {
            let rb = self.expr_to_any_reg(alloc, left, line)?;
            let dst = self.alloc_regs(1, line)?;
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::AddI, dst, rb, sc as u8),
                line,
            )?;
            return Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }));
        }

        let rb = self.expr_to_any_reg(alloc, left, line)?;
        let (opcode, rc) = self.arith_opcode_and_rc(alloc, op, right, line)?;
        let dst = self.alloc_regs(1, line)?;
        self.emit(alloc, Instruction::make_abc(opcode, dst, rb, rc), line)?;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    fn arith_opcode_and_rc(
        &mut self,
        alloc: &mut GcAllocator,
        op: BinOp,
        right: ExprDesc,
        line: u32,
    ) -> Result<(OpCode, u8), CompileError> {
        let opcode = match op {
            BinOp::Add => OpCode::Add,
            BinOp::Sub => OpCode::Sub,
            BinOp::Mul => OpCode::Mul,
            BinOp::Div => OpCode::Div,
            BinOp::Mod => OpCode::Mod,
            BinOp::Pow => OpCode::Pow,
            BinOp::Band => OpCode::BAnd,
            BinOp::Bor => OpCode::BOr,
            BinOp::Bxor => OpCode::BXor,
            BinOp::Shl => OpCode::Shl,
            BinOp::Shr => OpCode::Shr,
            _ => unreachable!("arith_opcode_and_rc only for arithmetic ops"),
        };

        if matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
        ) && let Some(rk) = self.try_rk_operand(alloc, right.kind, line)?
        {
            return Ok((opcode, rk));
        }

        let rc = self.expr_to_any_reg(alloc, right, line)?;
        Ok((opcode, rc))
    }

    fn emit_comparison(
        &mut self,
        alloc: &mut GcAllocator,
        op: BinOp,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        let (op, left, right, polarity) = normalize_comparison(op, left, right);
        let rb = self.expr_to_any_reg(alloc, left, line)?;

        let (cmp_op, rc) = if let Some(sc) = imm_sc(right.kind) {
            let i_op = match op {
                BinOp::Eq => OpCode::EqI,
                BinOp::Lt => OpCode::LtI,
                BinOp::Le => OpCode::LeI,
                _ => unreachable!("normalized compare is Eq/Lt/Le"),
            };
            (i_op, sc as u8)
        } else if let Some(rk) = self.try_rk_operand(alloc, right.kind, line)? {
            let r_op = match op {
                BinOp::Eq => OpCode::Eq,
                BinOp::Lt => OpCode::Lt,
                BinOp::Le => OpCode::Le,
                _ => unreachable!("normalized compare is Eq/Lt/Le"),
            };
            (r_op, rk)
        } else {
            let rc = self.expr_to_any_reg(alloc, right, line)?;
            let r_op = match op {
                BinOp::Eq => OpCode::Eq,
                BinOp::Lt => OpCode::Lt,
                BinOp::Le => OpCode::Le,
                _ => unreachable!("normalized compare is Eq/Lt/Le"),
            };
            (r_op, rc)
        };

        self.emit(alloc, Instruction::make_abc(cmp_op, polarity, rb, rc), line)?;
        let jmp_pc = self.emit_jmp_link(alloc, NO_JUMP, line)?;
        Ok(ExprDesc {
            kind: ExprKind::Jump { pc: jmp_pc },
            patch_true: jmp_pc,
            patch_false: NO_JUMP,
        })
    }

    fn try_rk_operand(
        &mut self,
        alloc: &mut GcAllocator,
        kind: ExprKind,
        line: u32,
    ) -> Result<Option<u8>, CompileError> {
        match kind {
            ExprKind::Const(k) => Ok(encode_rk(k)),
            ExprKind::Nil | ExprKind::True | ExprKind::False | ExprKind::IntImm(_) => {
                if let Some(val) = self.expr_to_const_value(kind) {
                    let k =
                        unsafe { (*self.proto).add_constant(alloc, &mut *self.const_dedup, val) }
                            .map_err(|e| CompileError::new(e, line))?;
                    Ok(encode_rk(k))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn expr_to_const_value(&self, kind: ExprKind) -> Option<Value> {
        match kind {
            ExprKind::Nil => Some(Value::NIL),
            ExprKind::True => Some(Value::TRUE),
            ExprKind::False => Some(Value::FALSE),
            ExprKind::IntImm(i) => Some(Value::from_i48(i64::from(i))),
            ExprKind::Const(k) => {
                // SAFETY: `k` is a valid constant pool index for `self.proto`.
                Some(unsafe { *(*self.proto).constants.add(usize::from(k)) })
            }
            _ => None,
        }
    }
}

fn imm_sc(kind: ExprKind) -> Option<i8> {
    match kind {
        ExprKind::IntImm(i) => encode_sc(i64::from(i)),
        _ => None,
    }
}

fn normalize_comparison(
    op: BinOp,
    left: ExprDesc,
    right: ExprDesc,
) -> (BinOp, ExprDesc, ExprDesc, u8) {
    match op {
        BinOp::Eq => (BinOp::Eq, left, right, 1),
        BinOp::Ne => (BinOp::Eq, left, right, 0),
        BinOp::Lt => (BinOp::Lt, left, right, 1),
        BinOp::Le => (BinOp::Le, left, right, 1),
        BinOp::Gt => (BinOp::Lt, right, left, 1),
        BinOp::Ge => (BinOp::Le, right, left, 1),
        _ => unreachable!("normalize_comparison only for relational ops"),
    }
}
