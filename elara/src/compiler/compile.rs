//! Source → `ObjProto` compilation entry points and script environment binding.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::fmt::Write as _;
use core::ptr::NonNull;

use crate::compiler::opcode::Instruction;
use crate::compiler::parse::Parser;
use crate::compiler::proto::{ObjProto, UpvalDesc};
use crate::compiler::scope::{CompileError, FunctionScope, INTRINSIC_NAMES};
use crate::compiler::{OpCode, ensure_string_constant};
use crate::ffi::sandbox::{has_capability, module_required_cap};
use crate::value::{GcHeader, ObjTable, ObjType, Value};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::upvalue::{ObjClosure, ObjUpvalue};

const MODULE_INTRINSICS: [&str; 6] = ["math", "string", "table", "bit", "sys", "fiber"];
const FN_INTRINSICS: [&str; 8] = [
    "pcall",
    "panic",
    "type",
    "tostring",
    "tonumber",
    "print",
    "assert",
    "__register_close",
];

/// Compiles `source` into a root script `ObjProto` (with env upvalue 0 for globals).
pub unsafe fn compile_script(
    state: &mut ElaraState,
    chunk_name: &str,
    source: &str,
) -> Result<*mut ObjProto, CompileError> {
    let name = state
        .strings
        .new_string(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            chunk_name.as_bytes(),
        )
        .ok_or_else(|| CompileError::new("Out of memory", 1))?;
    let proto = ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, name.as_ptr())
        .ok_or_else(|| CompileError::new("Out of memory", 1))?;
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 16)
        .ok_or_else(|| CompileError::new("Out of memory", 1))?;

    // Env upvalue filled by [`bind_env_upvalue`] at load/eval time.
    (*proto.as_ptr())
        .add_upvalue(
            &mut state.alloc,
            UpvalDesc {
                index: 0,
                is_local: 0,
                is_const: 1,
                reserved: 0,
            },
        )
        .map_err(|e| CompileError::new(e, 1))?;

    let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
    emit_intrinsic_prologue(state, &mut scope)?;

    let line = parser_end_line(source);
    let mut parser = Parser::new(state, source, scope);
    parser.parse_chunk()?;
    let mut scope = parser.into_scope();
    scope.emit(
        &mut state.alloc,
        Instruction::make_abc(OpCode::Return, 0, 1, 0),
        line,
    )?;
    Ok(proto.as_ptr())
}

/// Instantiates a closure for `proto` and binds upvalue 0 to `state.globals` when present.
pub unsafe fn closure_with_env(
    state: &mut ElaraState,
    proto: *mut ObjProto,
) -> Result<NonNull<ObjClosure>, ElaraStatus> {
    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto)
        .ok_or(ElaraStatus::ErrOom)?;
    bind_env_upvalue(state, closure.as_ptr())?;
    Ok(closure)
}

/// Binds closed upvalue slot 0 of a top-level script closure to the globals table.
pub unsafe fn bind_env_upvalue(
    state: &mut ElaraState,
    closure: *mut ObjClosure,
) -> Result<(), ElaraStatus> {
    if closure.is_null() || (*closure).upvalue_count == 0 {
        return Ok(());
    }
    if state.globals.is_null() {
        let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 32)
            .ok_or(ElaraStatus::ErrOom)?;
        state.globals = t.as_ptr();
    }
    let uv = ObjUpvalue::new_open(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
        0,
    )
    .ok_or(ElaraStatus::ErrOom)?
    .as_ptr();
    let globals = Value::from_obj(
        NonNull::new(state.globals.cast::<GcHeader>()).ok_or(ElaraStatus::ErrRuntime)?,
        ObjType::Table,
    );
    (*uv).closed = globals;
    (*uv).location = &raw mut (*uv).closed;
    *(*closure).upvalues = uv;
    Ok(())
}

/// Writes a compile diagnostic into `state.error_buf`.
pub fn set_compile_error(state: &mut ElaraState, err: &CompileError) {
    state.error_buf = [0u8; 256];
    let len = {
        let mut w = BufWriter {
            buf: &mut state.error_buf,
            len: 0,
        };
        let _ = write!(w, "{}:{}", err.line, err.message);
        w.len
    };
    let end = len.min(255);
    state.error_buf[end] = 0;
    state.error_buf_len = end + 1;
    state.last_error = Some("__dynamic__");
}

struct BufWriter<'a> {
    buf: &'a mut [u8; 256],
    len: usize,
}

impl core::fmt::Write for BufWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = self.buf.len().saturating_sub(self.len + 1);
        let n = bytes.len().min(avail);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

unsafe fn emit_intrinsic_prologue(
    state: &mut ElaraState,
    scope: &mut FunctionScope<'_>,
) -> Result<(), CompileError> {
    debug_assert_eq!(
        INTRINSIC_NAMES.len(),
        MODULE_INTRINSICS.len() + FN_INTRINSICS.len()
    );
    for name in MODULE_INTRINSICS {
        if let Some(cap) = module_required_cap(name)
            && !has_capability(state, cap)
        {
            // Skip modules disabled by the current sandbox bitmask (e.g. `sys`).
            continue;
        }
        scope.compile_import(
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            name,
            None,
            1,
        )?;
    }
    for name in FN_INTRINSICS {
        let reg = scope.declare_local(name, true, false, 1)?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetUpval, reg, 0, 0),
            1,
        )?;
        let key_k = ensure_string_constant(
            scope,
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            name.as_bytes(),
            1,
        )?;
        let key_u8 =
            u8::try_from(key_k).map_err(|_| CompileError::new("field name K index > 255", 1))?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetField, reg, reg, key_u8),
            1,
        )?;
        scope.mark_initialized();
    }
    Ok(())
}

fn parser_end_line(source: &str) -> u32 {
    let mut line = 1u32;
    for b in source.bytes() {
        if b == b'\n' {
            line += 1;
        }
    }
    line
}
