//! Module chunk codegen: exports table prologue, `export` field stores, and `OP_IMPORT`.

use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::scope::{CompileError, FunctionScope};
use crate::gc::GcAllocator;
use crate::value::{GcHeader, InternPool, ObjType, Value};

/// Derives the default `import` local name from a module specifier.
///
/// Uses the basename after the last `/`, stripping a trailing `.ela` or `.elac` if present.
#[must_use]
pub fn module_basename(specifier: &str) -> &str {
    let base = specifier.rsplit('/').next().unwrap_or(specifier);
    base.strip_suffix(".elac")
        .or_else(|| base.strip_suffix(".ela"))
        .unwrap_or(base)
}

/// Interns `bytes` into `pool` and ensures the string is present in `scope`'s constant pool.
pub fn ensure_string_constant(
    scope: &mut FunctionScope<'_>,
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    pool: &mut InternPool,
    bytes: &[u8],
    line: u32,
) -> Result<u16, CompileError> {
    let s = pool
        .new_string(alloc, all_objects, 0, bytes)
        .ok_or_else(|| CompileError::new("Out of memory", line))?;
    let v = Value::from_obj(s.cast(), ObjType::String);
    // SAFETY: proto / const_dedup owned by the compiler.
    unsafe { (*scope.proto).add_constant(alloc, &mut *scope.const_dedup, v) }
        .map_err(|e| CompileError::new(e, line))
}

impl<'a> FunctionScope<'a> {
    /// Starts a module chunk: reserves `R[0]` for the exports table and emits
    /// `OP_NEWTABLE R[0], 0, 4`. Subsequent locals allocate from `R[1]`.
    pub fn begin_module(&mut self, alloc: &mut GcAllocator, line: u32) -> Result<(), CompileError> {
        if self.free_reg != 0 {
            return Err(CompileError::new(
                "begin_module requires an empty register allocator",
                line,
            ));
        }
        self.emit(
            alloc,
            Instruction::make_abc(OpCode::NewTable, 0, 0, 4),
            line,
        )?;
        self.free_reg = 1;
        // SAFETY: proto owned by the compiler.
        unsafe {
            if (*self.proto).max_stack_size < 2 {
                (*self.proto).max_stack_size = 2;
            }
        }
        Ok(())
    }

    /// Declares a local for `name`, then emits `OP_SETFIELD R[0], K[name], R[value_reg]`.
    ///
    /// When `declare` is true, allocates a new local register and moves `value_reg` into it
    /// (unless `value_reg` already is that local). Typically used as:
    /// `let r = alloc/expr; emit_export(..., "x", r, true/false)`.
    pub fn emit_export(
        &mut self,
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        pool: &mut InternPool,
        name: &str,
        value_reg: u8,
        line: u32,
    ) -> Result<(), CompileError> {
        let key_k = ensure_string_constant(self, alloc, all_objects, pool, name.as_bytes(), line)?;
        let key_u8 =
            u8::try_from(key_k).map_err(|_| CompileError::new("field name K index > 255", line))?;
        self.emit(
            alloc,
            Instruction::make_abc(OpCode::SetField, 0, key_u8, value_reg),
            line,
        )?;
        Ok(())
    }

    /// Declares a `const` local for the import alias and emits `OP_IMPORT R[alias], K[specifier]`.
    ///
    /// When `alias` is `None`, the local name is [`module_basename`]`(specifier)`.
    pub fn compile_import(
        &mut self,
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        pool: &mut InternPool,
        specifier: &'a str,
        alias: Option<&'a str>,
        line: u32,
    ) -> Result<u8, CompileError> {
        let local_name = alias.unwrap_or_else(|| module_basename(specifier));
        if local_name.is_empty() {
            return Err(CompileError::new("empty import alias", line));
        }
        let reg = self.declare_local(local_name, true, false, line)?;
        self.mark_initialized();

        let bx =
            ensure_string_constant(self, alloc, all_objects, pool, specifier.as_bytes(), line)?;
        self.emit(alloc, Instruction::make_abx(OpCode::Import, reg, bx), line)?;
        Ok(reg)
    }

    /// Ends a module chunk with `OP_RETURN R[0], 2` (one result: the exports table).
    pub fn finish_module(
        &mut self,
        alloc: &mut GcAllocator,
        line: u32,
    ) -> Result<(), CompileError> {
        self.emit(alloc, Instruction::make_abc(OpCode::Return, 0, 2, 0), line)?;
        Ok(())
    }
}
