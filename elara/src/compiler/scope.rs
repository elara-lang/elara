//! Static lexical scope management, zero-register `const` propagation, and upvalue resolution.

use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::proto::{ELARA_MAX_LOCALS, ELARA_MAX_REGISTERS, NO_JUMP, ObjProto, UpvalDesc};
use crate::gc::GcAllocator;
use crate::value::{ObjTable, Value};

/// Pre-registered intrinsic bindings available in module scope without explicit declaration.
pub const INTRINSIC_NAMES: [&str; 14] = [
    "math",
    "string",
    "table",
    "bit",
    "sys",
    "pcall",
    "panic",
    "type",
    "tostring",
    "tonumber",
    "print",
    "assert",
    "fiber",
    "__register_close",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    Void,        // No value
    Nil,         // Literal nil
    True,        // Literal true
    False,       // Literal false
    IntImm(i16), // 16-bit signed integer immediate (fits in sBx)
    Const(u16),  // Constant pool index K[idx]
    Local { reg: u8, is_const: bool },
    Upvalue { idx: u8, is_const: bool },
    Indexed { table_reg: u8, key_reg: u8 },
    Field { table_reg: u8, key_k: u8 },
    Jump { pc: u32 },      // Conditional comparison whose trailing OP_JMP is at `pc`
    Relocable { pc: u32 }, // Instruction at `pc` whose Dst register (A) is unpatched
    NonReloc { reg: u8 },  // Value already materialized in temporary register R[reg]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprDesc {
    pub kind: ExprKind,
    pub patch_true: u32, // Intrusive jump list of OP_JMPs taken when expression is truthy
    pub patch_false: u32, // Intrusive jump list of OP_JMPs taken when expression is falsy
}

impl ExprDesc {
    #[inline]
    #[must_use]
    pub const fn new(kind: ExprKind) -> Self {
        Self {
            kind,
            patch_true: NO_JUMP,
            patch_false: NO_JUMP,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Local<'a> {
    pub name: &'a str,
    pub depth: i16,                  // -1 while uninitialized in its own initializer
    pub reg: u8,                     // Allocated virtual register R[reg] (or 0xFF if inline const)
    pub is_const: bool,              // true if declared with `const`
    pub is_captured: bool,           // true if captured by any nested closure
    pub has_close: bool,             // true if declared with `defer` / `__close`
    pub inline_const: Option<Value>, // Compile-time constant value for zero-register const inlining
}

#[derive(Debug, Clone, Copy)]
pub struct LoopScope {
    pub continue_target_pc: u32,
    pub break_jump_list: u32, // Intrusive in-place jump list head (NO_JUMP when empty)
    pub scope_depth: u16,
    pub first_local_reg: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub line: u32,
}

impl CompileError {
    #[inline]
    #[must_use]
    pub fn new(message: impl Into<String>, line: u32) -> Self {
        Self {
            message: message.into(),
            line,
        }
    }
}

/// Per-function lexical scope and register allocator state.
pub struct FunctionScope<'a> {
    pub proto: *mut ObjProto,
    pub const_dedup: *mut ObjTable,
    pub locals: Vec<Local<'a>>,
    pub loops: Vec<LoopScope>,
    pub scope_depth: u16,
    pub free_reg: u8,
    pub last_target_pc: u32,
}

impl<'a> FunctionScope<'a> {
    /// Creates a new `FunctionScope` for `proto` with its constant deduplication table `const_dedup`.
    #[must_use]
    pub fn new(proto: *mut ObjProto, const_dedup: *mut ObjTable) -> Self {
        Self {
            proto,
            const_dedup,
            locals: Vec::with_capacity(16),
            loops: Vec::new(),
            scope_depth: 0,
            free_reg: 0,
            last_target_pc: 0,
        }
    }

    /// Allocates `count` contiguous virtual registers starting at `self.free_reg`, updating
    /// `proto.max_stack_size`.
    pub fn alloc_regs(&mut self, count: u8, line: u32) -> Result<u8, CompileError> {
        let base = self.free_reg;
        let next = usize::from(base) + usize::from(count);
        if next > ELARA_MAX_REGISTERS {
            return Err(CompileError::new(
                "Register overflow (max 255 registers per function)",
                line,
            ));
        }
        self.free_reg = next as u8;
        // SAFETY: `self.proto` is a valid `ObjProto` owned by the compiler.
        unsafe {
            if self.free_reg > (*self.proto).max_stack_size {
                (*self.proto).max_stack_size = self.free_reg;
            }
        }
        Ok(base)
    }

    /// Declares a new runtime-register local variable in the current scope with `depth = -1`
    /// (uninitialized) until `mark_initialized` is called after compiling its initializer.
    pub fn declare_local(
        &mut self,
        name: &'a str,
        is_const: bool,
        has_close: bool,
        line: u32,
    ) -> Result<u8, CompileError> {
        if self.locals.len() >= ELARA_MAX_LOCALS {
            return Err(CompileError::new(
                "Too many local variables (max 200)",
                line,
            ));
        }
        let reg = self.alloc_regs(1, line)?;
        self.locals.push(Local {
            name,
            depth: -1,
            reg,
            is_const,
            is_captured: false,
            has_close,
            inline_const: None,
        });
        Ok(reg)
    }

    /// Declares a zero-register compile-time `const` binding (`reg = 0xFF`, `inline_const = Some(val)`).
    /// Consumes 0 virtual registers and 0 upvalues across nested closures!
    pub fn declare_inline_const(
        &mut self,
        name: &'a str,
        val: Value,
        line: u32,
    ) -> Result<(), CompileError> {
        if self.locals.len() >= ELARA_MAX_LOCALS {
            return Err(CompileError::new(
                "Too many local variables (max 200)",
                line,
            ));
        }
        self.locals.push(Local {
            name,
            depth: self.scope_depth as i16,
            reg: 0xFF,
            is_const: true,
            is_captured: false,
            has_close: false,
            inline_const: Some(val),
        });
        Ok(())
    }

    /// Marks the most recently declared local variable initialized at the current `scope_depth`.
    #[inline]
    pub fn mark_initialized(&mut self) {
        if let Some(last) = self.locals.last_mut() {
            last.depth = self.scope_depth as i16;
        }
    }

    /// Enters a new nested `{ ... }` lexical block scope.
    #[inline]
    pub fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    /// Exits the current `{ ... }` lexical block scope, freeing registers of popped locals and
    /// emitting `OP_CLOSE R[first_affected_reg]` if any popped local was captured or marked `defer`.
    pub fn end_scope(
        &mut self,
        alloc: &mut GcAllocator,
        line: u32,
    ) -> Result<Option<u8>, CompileError> {
        let target_depth = (self.scope_depth - 1) as i16;
        let close_reg = self.find_close_reg_above_depth(target_depth);

        while let Some(last) = self.locals.last() {
            if last.depth <= target_depth {
                break;
            }
            let popped = self.locals.pop().expect("non-empty");
            if popped.reg != 0xFF && popped.reg < self.free_reg {
                self.free_reg = popped.reg;
            }
        }

        self.scope_depth -= 1;

        if let Some(reg) = close_reg {
            // SAFETY: `self.proto` is valid.
            unsafe {
                (*self.proto)
                    .emit_instruction(alloc, Instruction::make_abc(OpCode::Close, reg, 0, 0), line)
                    .map_err(|e| CompileError::new(e, line))?;
            }
        }
        Ok(close_reg)
    }

    /// Returns the lowest register `R[reg]` among locals at `depth > target_depth` that have
    /// `is_captured == true` or `has_close == true`.
    #[must_use]
    pub fn find_close_reg_above_depth(&self, target_depth: i16) -> Option<u8> {
        let mut first_close_reg: Option<u8> = None;
        for loc in self.locals.iter().rev() {
            if loc.depth <= target_depth {
                break;
            }
            if loc.reg != 0xFF && (loc.is_captured || loc.has_close) {
                first_close_reg = Some(match first_close_reg {
                    Some(cur) => cur.min(loc.reg),
                    None => loc.reg,
                });
            }
        }
        first_close_reg
    }

    /// Returns `true` if any active local in the function has `has_close == true` (`defer` / `__close`).
    /// When `true`, `return f(...)` MUST NOT be emitted as `OP_TAILCALL`.
    #[must_use]
    pub fn has_active_defer(&self) -> bool {
        self.locals.iter().any(|l| l.has_close)
    }

    /// Materializes a compile-time constant `Value` into an `ExprKind` (`Nil`, `True`, `False`,
    /// `IntImm(i16)`, or `Const(u16)` in `self.proto`).
    pub fn value_to_expr_kind(
        &mut self,
        alloc: &mut GcAllocator,
        val: Value,
        line: u32,
    ) -> Result<ExprKind, CompileError> {
        if val.is_nil() {
            return Ok(ExprKind::Nil);
        }
        if val.is_bool() {
            return Ok(if val.as_bool() {
                ExprKind::True
            } else {
                ExprKind::False
            });
        }
        if val.is_i48() {
            let i = val.as_i48();
            if let Ok(imm16) = i16::try_from(i) {
                return Ok(ExprKind::IntImm(imm16));
            }
        }
        // SAFETY: `self.proto` and `self.const_dedup` are valid pointers.
        let k_idx = unsafe { (*self.proto).add_constant(alloc, &mut *self.const_dedup, val) }
            .map_err(|e| CompileError::new(e, line))?;
        Ok(ExprKind::Const(k_idx))
    }
}

/// Resolves an identifier `name` across the stack of enclosing `FunctionScope`s (`scopes[0..=top]`),
/// propagating `inline_const` values with 0 registers/0 upvalues, capturing multi-level lexical
/// upvalues when needed, and rejecting undeclared identifiers.
pub fn resolve_variable<'a>(
    alloc: &mut GcAllocator,
    scopes: &mut [FunctionScope<'a>],
    name: &'a str,
    line: u32,
) -> Result<ExprKind, CompileError> {
    let top = scopes.len() - 1;
    resolve_at_level(alloc, scopes, top, name, line)
}

/// Verifies that an assignment to `name` does not mutate a `const` binding (`Local` or `Upvalue`),
/// returning the resolved `ExprKind` when mutable or `Err("Cannot assign to const variable '<name>'")`.
pub fn resolve_assignable<'a>(
    alloc: &mut GcAllocator,
    scopes: &mut [FunctionScope<'a>],
    name: &'a str,
    line: u32,
) -> Result<ExprKind, CompileError> {
    // First check if `name` is an inline const in any enclosing scope
    for scope in scopes.iter().rev() {
        for loc in scope.locals.iter().rev() {
            if loc.name == name && loc.depth != -1 {
                if loc.is_const {
                    return Err(CompileError::new(
                        format!("Cannot assign to const variable '{name}'"),
                        line,
                    ));
                }
                break;
            }
        }
    }

    let kind = resolve_variable(alloc, scopes, name, line)?;
    match kind {
        ExprKind::Local { is_const: true, .. } | ExprKind::Upvalue { is_const: true, .. } => Err(
            CompileError::new(format!("Cannot assign to const variable '{name}'"), line),
        ),
        _ => Ok(kind),
    }
}

fn resolve_at_level<'a>(
    alloc: &mut GcAllocator,
    scopes: &mut [FunctionScope<'a>],
    level: usize,
    name: &'a str,
    line: u32,
) -> Result<ExprKind, CompileError> {
    // 1. Search locals in `scopes[level]` from innermost to outermost
    let found_local = {
        let scope = &scopes[level];
        let mut res = None;
        for (i, loc) in scope.locals.iter().enumerate().rev() {
            if loc.name == name {
                res = Some((i, *loc));
                break;
            }
        }
        res
    };

    if let Some((_idx, loc)) = found_local {
        if loc.depth == -1 {
            return Err(CompileError::new(
                "Cannot read local variable in its own initializer",
                line,
            ));
        }
        if let Some(val) = loc.inline_const {
            return scopes[level].value_to_expr_kind(alloc, val, line);
        }
        return Ok(ExprKind::Local {
            reg: loc.reg,
            is_const: loc.is_const,
        });
    }

    // 2. If at top-level module scope (`level == 0`), reject undeclared identifiers
    if level == 0 {
        return Err(CompileError::new(
            format!("Undeclared identifier '{name}'"),
            line,
        ));
    }

    // 3. Search enclosing function scope (`level - 1`) recursively
    let parent_kind = resolve_at_level(alloc, scopes, level - 1, name, line)?;
    match parent_kind {
        // Zero-register `const` propagation across closure boundaries (0 upvalues!)
        ExprKind::Nil | ExprKind::True | ExprKind::False | ExprKind::IntImm(_) => Ok(parent_kind),
        ExprKind::Const(parent_k) => {
            // Copy constant from parent proto's K[parent_k] into current proto's K
            let val = unsafe {
                let p_proto = scopes[level - 1].proto;
                *(*p_proto).constants.add(usize::from(parent_k))
            };
            scopes[level].value_to_expr_kind(alloc, val, line)
        }
        ExprKind::Local { reg, is_const } => {
            // Mark parent's local as captured!
            for loc in scopes[level - 1].locals.iter_mut().rev() {
                if loc.reg == reg && loc.name == name {
                    loc.is_captured = true;
                    break;
                }
            }
            let desc = UpvalDesc {
                index: reg,
                is_local: 1,
                is_const: u8::from(is_const),
                reserved: 0,
            };
            let upval_idx = unsafe { (*scopes[level].proto).add_upvalue(alloc, desc) }
                .map_err(|e| CompileError::new(e, line))?;
            Ok(ExprKind::Upvalue {
                idx: upval_idx,
                is_const,
            })
        }
        ExprKind::Upvalue { idx, is_const } => {
            let desc = UpvalDesc {
                index: idx,
                is_local: 0,
                is_const: u8::from(is_const),
                reserved: 0,
            };
            let upval_idx = unsafe { (*scopes[level].proto).add_upvalue(alloc, desc) }
                .map_err(|e| CompileError::new(e, line))?;
            Ok(ExprKind::Upvalue {
                idx: upval_idx,
                is_const,
            })
        }
        _ => Err(CompileError::new(
            format!("Undeclared identifier '{name}'"),
            line,
        )),
    }
}
