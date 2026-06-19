//! Module cache, cycle detection, capability gating, and `OP_IMPORT` execution.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::fmt::Write;
use std::ffi::CString;

use crate::ffi::sandbox::{has_capability, module_required_cap};
use crate::value::nan::{ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::closure::ObjClosure;
use crate::vm::dispatch::execute;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::verify::deserialize_elac;

/// Light-userdata sentinel written into `modules_table` while a module is loading.
pub const MODULE_LOADING_SENTINEL: Value = Value(0x7FFF_0000_0000_0001);

struct ErrBufWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Write for ErrBufWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let space = self.buf.len().saturating_sub(1).saturating_sub(self.len);
        let n = s.len().min(space);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Formats a dynamic module error into `state.error_buf`.
pub fn set_module_error(state: &mut ElaraState, args: core::fmt::Arguments<'_>) {
    state.error_buf = [0u8; 256];
    let len = {
        let mut w = ErrBufWriter {
            buf: &mut state.error_buf,
            len: 0,
        };
        let _ = w.write_fmt(args);
        w.len
    };
    state.error_buf[len] = 0;
    state.error_buf_len = len + 1;
    state.last_error = Some("__dynamic__");
    state.error.last_status = ElaraStatus::ErrModule;
}

fn clear_dynamic_error(state: &mut ElaraState) {
    if state.error_buf_len > 0 {
        state.error_buf_len = 0;
        if state.last_error == Some("__dynamic__") {
            state.last_error = None;
        }
    }
}

/// Ensures `state.modules_table` exists (lazy allocation).
pub unsafe fn ensure_modules_table(state: &mut ElaraState) -> Result<*mut ObjTable, ElaraStatus> {
    if !state.modules_table.is_null() {
        return Ok(state.modules_table);
    }
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 16)
        .ok_or(ElaraStatus::ErrOom)?;
    state.modules_table = t.as_ptr();
    Ok(state.modules_table)
}

/// Executes `OP_IMPORT A, Bx` — resolves `K[Bx]` as a module specifier into `R[A]`.
pub unsafe fn op_import(
    state: &mut ElaraState,
    base: *mut Value,
    k: *const Value,
    a: u8,
    bx: u16,
) -> Result<(), ElaraStatus> {
    let spec_val = *k.add(bx as usize);
    if !spec_val.is_obj_type(ObjType::String) {
        set_module_error(state, format_args!("Module specifier must be a string"));
        return Err(ElaraStatus::ErrModule);
    }
    let spec = (*spec_val.as_obj().cast::<ObjString>()).as_str();
    let table = ensure_modules_table(state)?;

    // 1. Cache lookup
    let cached = (*table).get(spec_val);
    if !cached.is_nil() {
        if cached.0 == MODULE_LOADING_SENTINEL.0 {
            set_module_error(
                state,
                format_args!("Circular module dependency detected: '{spec}'"),
            );
            return Err(ElaraStatus::ErrModule);
        }
        if cached.is_obj_type(ObjType::Table) {
            *base.add(a as usize) = cached;
            return Ok(());
        }
    }

    // 2. Intrinsic registry (empty tables until stdlib bodies land).
    if matches!(spec, "math" | "string" | "table" | "bit" | "sys" | "fiber") {
        if let Some(cap) = module_required_cap(spec)
            && !has_capability(state, cap)
        {
            set_module_error(
                state,
                format_args!("Module '{spec}' is disabled by sandbox capabilities"),
            );
            return Err(ElaraStatus::ErrModule);
        }
        let exports = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4)
            .ok_or(ElaraStatus::ErrOom)?;
        let ev = Value::from_obj(
            core::ptr::NonNull::new(exports.as_ptr().cast()).unwrap(),
            ObjType::Table,
        );
        (*table)
            .set(&mut state.alloc, spec_val, ev)
            .map_err(|_| ElaraStatus::ErrOom)?;
        *base.add(a as usize) = ev;
        return Ok(());
    }

    // Path-traversal rejection (filesystem / host paths).
    if spec.split('/').any(|seg| seg == "..") {
        set_module_error(state, format_args!("Module path traversal rejected"));
        return Err(ElaraStatus::ErrModule);
    }

    // 3. Host custom loader
    let Some(loader) = state.module_loader else {
        set_module_error(state, format_args!("Module not found: '{spec}'"));
        return Err(ElaraStatus::ErrModule);
    };

    (*table)
        .set(&mut state.alloc, spec_val, MODULE_LOADING_SENTINEL)
        .map_err(|_| ElaraStatus::ErrOom)?;

    let c_spec = match CString::new(spec) {
        Ok(s) => s,
        Err(_) => {
            remove_cache_entry(state, table, spec_val);
            set_module_error(state, format_args!("Invalid module specifier"));
            return Err(ElaraStatus::ErrModule);
        }
    };

    let mut out_bytes: *const u8 = core::ptr::null();
    let mut out_len: usize = 0;
    let mut out_is_bc = false;
    let st = loader(
        state as *mut ElaraState,
        c_spec.as_ptr(),
        &mut out_bytes,
        &mut out_len,
        &mut out_is_bc,
        state.module_loader_ud,
    );

    if st != ElaraStatus::Ok || out_bytes.is_null() || out_len == 0 {
        remove_cache_entry(state, table, spec_val);
        if state.error_buf_len == 0
            && (state.last_error.is_none() || state.last_error == Some("__dynamic__"))
        {
            set_module_error(state, format_args!("Module not found: '{spec}'"));
        }
        return Err(if st == ElaraStatus::Ok {
            ElaraStatus::ErrModule
        } else {
            st
        });
    }

    let bytes = core::slice::from_raw_parts(out_bytes, out_len);
    if !out_is_bc {
        remove_cache_entry(state, table, spec_val);
        set_module_error(
            state,
            format_args!("source module loading requires the compiler pipeline"),
        );
        return Err(ElaraStatus::ErrModule);
    }

    match load_and_run_bytecode(state, bytes) {
        Ok(exports) => {
            (*table)
                .set(&mut state.alloc, spec_val, exports)
                .map_err(|_| ElaraStatus::ErrOom)?;
            *base.add(a as usize) = exports;
            clear_dynamic_error(state);
            Ok(())
        }
        Err(e) => {
            remove_cache_entry(state, table, spec_val);
            Err(e)
        }
    }
}

unsafe fn remove_cache_entry(state: &mut ElaraState, table: *mut ObjTable, key: Value) {
    let _ = (*table).set(&mut state.alloc, key, Value::NIL);
}

unsafe fn load_and_run_bytecode(
    state: &mut ElaraState,
    bytes: &[u8],
) -> Result<Value, ElaraStatus> {
    let proto = deserialize_elac(state, bytes)?;
    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto)
        .ok_or(ElaraStatus::ErrOom)?;
    let stack_slots = (1 + (*proto).max_stack_size as u32).max(16);
    let mut fiber = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, stack_slots, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    let f = fiber.as_mut();
    let cv = Value::from_obj(
        core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
        ObjType::Closure,
    );
    *f.stack = cv;
    f.open_top = 1;
    f.push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))?;

    let saved = state.current_fiber;
    state.current_fiber = fiber.as_ptr();
    let st = execute(state, fiber.as_ptr());
    state.current_fiber = saved;

    let out = if st == ElaraStatus::Ok {
        let v = *f.stack;
        if !v.is_obj_type(ObjType::Table) {
            set_module_error(
                state,
                format_args!("Module did not return an exports table"),
            );
            Err(ElaraStatus::ErrModule)
        } else {
            Ok(v)
        }
    } else {
        Err(st)
    };

    let fiber_ptr = fiber.as_ptr();
    unlink_object(&mut state.all_objects, fiber_ptr.cast());
    (*fiber_ptr).free_fiber(&mut state.alloc);
    out
}

unsafe fn unlink_object(
    head: &mut *mut crate::value::GcHeader,
    target: *mut crate::value::GcHeader,
) {
    let mut cursor = head;
    while !(*cursor).is_null() {
        if *cursor == target {
            *cursor = (*target).next;
            return;
        }
        cursor = &mut (*(*cursor)).next;
    }
}
