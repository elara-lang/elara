//! Statement emission: compound assignment, table sizing, range-for, and defer-aware returns.

use crate::compiler::expr::{BinOp, encode_rk, encode_sc};
use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::scope::{CompileError, ExprDesc, ExprKind, FunctionScope, LoopScope};
use crate::gc::GcAllocator;
use crate::value::Value;

/// Result of emitting a specialized numeric range `for` header (`OP_FORPREP` / `OP_FORLOOP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeForInfo {
    pub loop_base: u8,
    pub forprep_pc: u32,
    pub forloop_pc: u32,
}

/// In-progress table literal being sized for exact `OP_NEWTABLE` capacity backpatching.
#[derive(Debug, Clone, Copy)]
pub struct TableBuilder {
    pub dst: u8,
    pub newtable_pc: u32,
    pub array_items: u32,
    pub hash_items: u32,
}

/// Computes `C = ceil_log2(hash_items * 4 / 3)` for `OP_NEWTABLE` hash-part sizing.
#[inline]
#[must_use]
pub fn newtable_hash_log2(hash_items: u32) -> u8 {
    if hash_items == 0 {
        return 0;
    }
    let slots = (u64::from(hash_items) * 4).div_ceil(3);
    let slots = u32::try_from(slots).unwrap_or(u32::MAX);
    ceil_log2(slots)
}

/// Ceiling of `log2(n)` for `n >= 1` (`0` when `n <= 1`).
#[inline]
#[must_use]
pub const fn ceil_log2(n: u32) -> u8 {
    if n <= 1 { 0 } else { (n - 1).ilog2() as u8 + 1 }
}

impl<'a> FunctionScope<'a> {
    /// Returns the lowest register among active `has_close` locals, if any.
    #[must_use]
    pub fn first_defer_reg(&self) -> Option<u8> {
        self.locals
            .iter()
            .filter(|l| l.has_close && l.reg != 0xFF)
            .map(|l| l.reg)
            .min()
    }

    /// Emits `OP_NEWTABLE R[dst], 0, 0` and returns a builder that counts array/hash items.
    pub fn begin_table(
        &mut self,
        alloc: &mut GcAllocator,
        dst: u8,
        line: u32,
    ) -> Result<TableBuilder, CompileError> {
        let newtable_pc = self.emit(
            alloc,
            Instruction::make_abc(OpCode::NewTable, dst, 0, 0),
            line,
        )?;
        Ok(TableBuilder {
            dst,
            newtable_pc,
            array_items: 0,
            hash_items: 0,
        })
    }

    /// Records `count` array elements and emits `OP_SETLIST` for a contiguous batch starting at
    /// `R[dst + 1]` with base index `array_base` (typically `builder.array_items` before the batch).
    pub fn table_setlist(
        &mut self,
        alloc: &mut GcAllocator,
        builder: &mut TableBuilder,
        batch_len: u8,
        array_base: u32,
        line: u32,
    ) -> Result<(), CompileError> {
        // `OP_SETLIST A, B, C`: for i in 0..B: R[A][C*64 + i] = R[A + 1 + i]
        let c = (array_base / 64) as u8;
        self.emit(
            alloc,
            Instruction::make_abc(OpCode::SetList, builder.dst, batch_len, c),
            line,
        )?;
        builder.array_items = builder.array_items.saturating_add(u32::from(batch_len));
        Ok(())
    }

    /// Records one map/hash field written via `OP_SETTAB R[dst][R[key]] = RK(val)`.
    pub fn table_settab(
        &mut self,
        alloc: &mut GcAllocator,
        builder: &mut TableBuilder,
        key_reg: u8,
        val_rk: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        self.emit(
            alloc,
            Instruction::make_abc(OpCode::SetTab, builder.dst, key_reg, val_rk),
            line,
        )?;
        builder.hash_items = builder.hash_items.saturating_add(1);
        Ok(())
    }

    /// Records one map field written via `OP_SETFIELD R[dst][K[key_k]] = R[val]`.
    pub fn table_setfield(
        &mut self,
        alloc: &mut GcAllocator,
        builder: &mut TableBuilder,
        key_k: u8,
        val_reg: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        self.emit(
            alloc,
            Instruction::make_abc(OpCode::SetField, builder.dst, key_k, val_reg),
            line,
        )?;
        builder.hash_items = builder.hash_items.saturating_add(1);
        Ok(())
    }

    /// Backpatches `OP_NEWTABLE` at `builder.newtable_pc` with exact array/hash capacity hints.
    pub fn finish_table(&mut self, builder: TableBuilder) {
        let b = builder.array_items.min(255) as u8;
        let c = newtable_hash_log2(builder.hash_items);
        // SAFETY: `newtable_pc` indexes the `OP_NEWTABLE` emitted by `begin_table`.
        unsafe {
            (*(*self.proto).code.add(builder.newtable_pc as usize)).patch_b_c(b, c);
        }
    }

    /// Compiles `<lhs> <op>= <rhs>` with single evaluation of indexed/field table and key registers.
    pub fn compile_compound_assign(
        &mut self,
        alloc: &mut GcAllocator,
        lhs: ExprDesc,
        op: BinOp,
        rhs: ExprDesc,
        line: u32,
    ) -> Result<(), CompileError> {
        match lhs.kind {
            ExprKind::Local {
                reg,
                is_const: false,
            } => self.emit_inplace_binop(alloc, reg, op, rhs, line),
            ExprKind::Local { is_const: true, .. } | ExprKind::Upvalue { is_const: true, .. } => {
                Err(CompileError::new("Cannot assign to const variable", line))
            }
            ExprKind::Indexed { table_reg, key_reg } => {
                // table_reg / key_reg already evaluated exactly once by the caller.
                let tmp = self.alloc_regs(1, line)?;
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::GetTab, tmp, table_reg, key_reg),
                    line,
                )?;
                self.emit_inplace_binop(alloc, tmp, op, rhs, line)?;
                // Store back: SETTAB R[table][R[key]] = RK(tmp) — encode tmp as register C.
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::SetTab, table_reg, key_reg, tmp),
                    line,
                )?;
                self.free_reg = tmp;
                Ok(())
            }
            ExprKind::Field { table_reg, key_k } => {
                let tmp = self.alloc_regs(1, line)?;
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::GetField, tmp, table_reg, key_k),
                    line,
                )?;
                self.emit_inplace_binop(alloc, tmp, op, rhs, line)?;
                // SETFIELD R[table][K[key_k]] = R[tmp]
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::SetField, table_reg, key_k, tmp),
                    line,
                )?;
                self.free_reg = tmp;
                Ok(())
            }
            _ => Err(CompileError::new("Invalid assignment target", line)),
        }
    }

    /// Emits a zero-allocation numeric range `for` over `[start, end)` (or inclusive end when
    /// `inclusive`), using `OP_FORPREP` + `OP_FORLOOP` without `OP_RANGE`.
    ///
    /// Layout: `R[loop_base]=start`, `R[loop_base+1]=end`, `R[loop_base+2]=step(1)`,
    /// `R[loop_base+3]=loop variable`. `emit_body` receives the loop-variable register.
    pub fn compile_range_for<F>(
        &mut self,
        alloc: &mut GcAllocator,
        start: ExprDesc,
        end: ExprDesc,
        inclusive: bool,
        line: u32,
        emit_body: F,
    ) -> Result<RangeForInfo, CompileError>
    where
        F: FnOnce(&mut Self, &mut GcAllocator, u8) -> Result<(), CompileError>,
    {
        let loop_base = self.alloc_regs(4, line)?;
        self.discharge_to_reg(alloc, start, loop_base, line)?;
        self.discharge_to_reg(alloc, end, loop_base + 1, line)?;

        // Inclusive `..=` adjusts end by +1 via ADDI when possible so FORLOOP's exclusive bound works.
        if inclusive {
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::AddI, loop_base + 1, loop_base + 1, 1u8),
                line,
            )?;
        }

        self.emit(
            alloc,
            Instruction::make_asbx(OpCode::LoadI, loop_base + 2, 1),
            line,
        )?;

        let forprep_pc = self.emit(
            alloc,
            Instruction::make_asbx(OpCode::ForPrep, loop_base, 0),
            line,
        )?;

        let scope_depth = self.scope_depth;
        self.loops.push(LoopScope {
            continue_target_pc: forprep_pc + 1,
            break_jump_list: crate::compiler::proto::NO_JUMP,
            scope_depth,
            first_local_reg: loop_base,
        });

        emit_body(self, alloc, loop_base + 3)?;

        let forloop_pc = self.emit(
            alloc,
            Instruction::make_asbx(OpCode::ForLoop, loop_base, 0),
            line,
        )?;

        // FORLOOP jumps backward to the first body instruction (forprep_pc + 1).
        let body_pc = forprep_pc + 1;
        let forloop_sbx = body_pc as i32 - (forloop_pc as i32 + 1);
        // SAFETY: `forloop_pc` indexes the just-emitted `OP_FORLOOP`.
        unsafe {
            *(*self.proto).code.add(forloop_pc as usize) =
                Instruction::make_asbx(OpCode::ForLoop, loop_base, forloop_sbx);
        }

        // FORPREP jumps forward to the instruction after FORLOOP.
        let after = self.pc();
        let forprep_sbx = after as i32 - (forprep_pc as i32 + 1);
        unsafe {
            *(*self.proto).code.add(forprep_pc as usize) =
                Instruction::make_asbx(OpCode::ForPrep, loop_base, forprep_sbx);
        }

        if let Some(loop_scope) = self.loops.pop() {
            self.patch_jump_list_to_here(loop_scope.break_jump_list);
        }

        // Mark FORLOOP as a jump target for peephole safety.
        if forloop_pc + 1 > self.last_target_pc {
            self.last_target_pc = after;
        }

        Ok(RangeForInfo {
            loop_base,
            forprep_pc,
            forloop_pc,
        })
    }

    /// Compiles `return f(...)` as `OP_TAILCALL` when no `defer` is active; otherwise
    /// `OP_CALL` + `OP_CLOSE` + `OP_RETURN`.
    pub fn compile_return_call(
        &mut self,
        alloc: &mut GcAllocator,
        func_reg: u8,
        num_args: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        let b = num_args.saturating_add(1); // B-1 == num_args
        if self.has_active_defer() {
            // CALL with C=2 expects 1 result left in R[func_reg].
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::Call, func_reg, b, 2),
                line,
            )?;
            if let Some(close_reg) = self.first_defer_reg() {
                self.emit(
                    alloc,
                    Instruction::make_abc(OpCode::Close, close_reg, 0, 0),
                    line,
                )?;
            }
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::Return, func_reg, 2, 0),
                line,
            )?;
        } else {
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::TailCall, func_reg, b, 0),
                line,
            )?;
        }
        Ok(())
    }

    /// Emits `break`, linking an `OP_JMP` onto the innermost loop's `break_jump_list`.
    pub fn compile_break(
        &mut self,
        alloc: &mut GcAllocator,
        line: u32,
    ) -> Result<(), CompileError> {
        let prev = self
            .loops
            .last()
            .map(|l| l.break_jump_list)
            .ok_or_else(|| CompileError::new("'break' outside of loop", line))?;
        let jmp = self.emit_jmp_link(alloc, prev, line)?;
        if let Some(loop_scope) = self.loops.last_mut() {
            loop_scope.break_jump_list = jmp;
        }
        Ok(())
    }

    fn emit_inplace_binop(
        &mut self,
        alloc: &mut GcAllocator,
        dst: u8,
        op: BinOp,
        rhs: ExprDesc,
        line: u32,
    ) -> Result<(), CompileError> {
        if op == BinOp::Add
            && let Some(sc) = match rhs.kind {
                ExprKind::IntImm(i) => encode_sc(i64::from(i)),
                _ => None,
            }
        {
            self.emit(
                alloc,
                Instruction::make_abc(OpCode::AddI, dst, dst, sc as u8),
                line,
            )?;
            return Ok(());
        }

        let opcode = match op {
            BinOp::Add => OpCode::Add,
            BinOp::Sub => OpCode::Sub,
            BinOp::Mul => OpCode::Mul,
            BinOp::Div => OpCode::Div,
            BinOp::Mod => OpCode::Mod,
            BinOp::Band => OpCode::BAnd,
            BinOp::Bor => OpCode::BOr,
            BinOp::Bxor => OpCode::BXor,
            BinOp::Shl => OpCode::Shl,
            BinOp::Shr => OpCode::Shr,
            _ => {
                return Err(CompileError::new(
                    "Unsupported compound assignment operator",
                    line,
                ));
            }
        };

        let rc = match rhs.kind {
            ExprKind::Const(k) => encode_rk(k).ok_or_else(|| {
                CompileError::new("Constant index exceeds RK(C) range (max 127)", line)
            })?,
            ExprKind::Nil | ExprKind::True | ExprKind::False | ExprKind::IntImm(_) => {
                let val = match rhs.kind {
                    ExprKind::Nil => Value::NIL,
                    ExprKind::True => Value::TRUE,
                    ExprKind::False => Value::FALSE,
                    ExprKind::IntImm(i) => Value::from_i48(i64::from(i)),
                    _ => unreachable!(),
                };
                let k = unsafe { (*self.proto).add_constant(alloc, &mut *self.const_dedup, val) }
                    .map_err(|e| CompileError::new(e, line))?;
                if let Some(rk) = encode_rk(k) {
                    rk
                } else {
                    self.expr_to_any_reg(alloc, rhs, line)?
                }
            }
            _ => self.expr_to_any_reg(alloc, rhs, line)?,
        };

        self.emit(alloc, Instruction::make_abc(opcode, dst, dst, rc), line)?;
        Ok(())
    }
}
