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
use crate::vm::library::{self, LoadFrame, SpecFail};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

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
    let raw_val = *k.add(bx as usize);
    if !raw_val.is_obj_type(ObjType::String) {
        set_module_error(state, format_args!("Module specifier must be a string"));
        return Err(ElaraStatus::ErrModule);
    }
    let raw = (*raw_val.as_obj().cast::<ObjString>()).as_str();
    let frame = library::active_frame(state.module_anchor.as_deref(), &state.module_load_stack);
    let logical = match library::classify(raw, frame.as_ref()) {
        Ok(spec) => spec,
        Err(fail) => {
            set_spec_fail(state, raw, fail);
            return Err(ElaraStatus::ErrModule);
        }
    };
    let table = ensure_modules_table(state)?;
    let key_val = intern_spec(state, &logical.key)?;

    let cached = (*table).get(key_val);
    if !cached.is_nil() {
        if cached.0 == MODULE_LOADING_SENTINEL.0 {
            set_module_error(
                state,
                format_args!("Circular module dependency detected: '{}'", logical.key),
            );
            return Err(ElaraStatus::ErrModule);
        }
        if cached.is_obj_type(ObjType::Table) {
            *base.add(a as usize) = cached;
            return Ok(());
        }
    }

    if is_intrinsic(&logical.key) {
        return load_intrinsic(state, table, key_val, base, a, &logical.key);
    }

    (*table)
        .set(&mut state.alloc, key_val, MODULE_LOADING_SENTINEL)
        .map_err(|_| ElaraStatus::ErrOom)?;

    if let Some(result) = crate::vm::native_lib::open_registered(state, &logical.key) {
        return finish_module_load(state, table, key_val, base, a, result);
    }

    if let Some(loader) = state.module_loader {
        let c_spec = match CString::new(logical.key.clone()) {
            Ok(s) => s,
            Err(_) => {
                remove_cache_entry(state, table, key_val);
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

        if st == ElaraStatus::Ok && !out_bytes.is_null() && out_len > 0 {
            let bytes = core::slice::from_raw_parts(out_bytes, out_len);
            let result = if out_is_bc {
                let frame = byte_frame(state, &logical.key);
                state.module_load_stack.push(frame);
                let result = crate::vm::module_run::load_and_run_bytecode(state, bytes);
                state.module_load_stack.pop();
                result
            } else {
                let src = match core::str::from_utf8(bytes) {
                    Ok(s) => s.to_owned(),
                    Err(_) => {
                        remove_cache_entry(state, table, key_val);
                        set_module_error(state, format_args!("Module source is not valid UTF-8"));
                        return Err(ElaraStatus::ErrModule);
                    }
                };
                let frame = byte_frame(state, &logical.key);
                state.module_load_stack.push(frame);
                let result =
                    crate::vm::module_run::compile_and_run_module_source(state, &logical.key, &src);
                state.module_load_stack.pop();
                result
            };
            return finish_module_load(state, table, key_val, base, a, result);
        }
    }

    let read_files = has_capability(state, crate::ffi::sandbox::ELARA_CAP_SYS_ENV);
    let read_native = has_capability(state, crate::ffi::sandbox::ELARA_CAP_NATIVE);
    if (read_files || read_native)
        && let Some(found) = library::find_module(
            &state.module_search_dirs,
            &logical.key,
            logical.only_root.as_deref(),
            read_files,
            read_native,
        )
    {
        let frame = LoadFrame {
            search_root: Some(found.search_root.clone()),
            rel_dir: found.rel_dir.clone(),
        };
        if found.is_native {
            state.module_load_stack.push(frame);
            let result = crate::vm::native_lib::open_shared(state, &found.path, &logical.key);
            state.module_load_stack.pop();
            return finish_module_load(state, table, key_val, base, a, result);
        }
        let result = if found.is_bytecode {
            state.module_load_stack.push(frame);
            let result = crate::vm::module_run::load_and_run_bytecode(state, &found.bytes);
            state.module_load_stack.pop();
            result
        } else {
            let src_owned = match core::str::from_utf8(&found.bytes) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    remove_cache_entry(state, table, key_val);
                    set_module_error(state, format_args!("Module source is not valid UTF-8"));
                    return Err(ElaraStatus::ErrModule);
                }
            };
            state.module_load_stack.push(frame);
            let result = crate::vm::module_run::compile_and_run_module_source(
                state,
                &logical.key,
                &src_owned,
            );
            state.module_load_stack.pop();
            result
        };
        state.module_load_buf = found.bytes;
        return finish_module_load(state, table, key_val, base, a, result);
    }

    remove_cache_entry(state, table, key_val);
    set_module_error(state, format_args!("Module not found: '{}'", logical.key));
    Err(ElaraStatus::ErrModule)
}

fn is_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "math" | "string" | "table" | "bit" | "sys" | "fiber" | "gc"
    )
}

fn set_spec_fail(state: &mut ElaraState, raw: &str, fail: SpecFail) {
    match fail {
        SpecFail::Absolute => {
            set_module_error(state, format_args!("Module path is absolute: '{raw}'"));
        }
        SpecFail::Invalid => {
            set_module_error(state, format_args!("Invalid module specifier"));
        }
        SpecFail::NeedsAnchor => {
            set_module_error(
                state,
                format_args!("Relative import requires a file anchor"),
            );
        }
        SpecFail::Traversal => {
            set_module_error(state, format_args!("Module path traversal rejected"));
        }
    }
}

unsafe fn intern_spec(state: &mut ElaraState, key: &str) -> Result<Value, ElaraStatus> {
    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, key.as_bytes())
        .ok_or(ElaraStatus::ErrOom)?;
    Ok(Value::from_obj(s.cast(), ObjType::String))
}

unsafe fn load_intrinsic(
    state: &mut ElaraState,
    table: *mut ObjTable,
    key_val: Value,
    base: *mut Value,
    a: u8,
    key: &str,
) -> Result<(), ElaraStatus> {
    if let Some(cap) = module_required_cap(key)
        && !has_capability(state, cap)
    {
        set_module_error(
            state,
            format_args!("Module '{key}' is disabled by sandbox capabilities"),
        );
        return Err(ElaraStatus::ErrModule);
    }
    let ev = match crate::stdlib::intrinsic_module(state, key)? {
        Some(v) => v,
        None => {
            set_module_error(state, format_args!("Module not found: '{key}'"));
            return Err(ElaraStatus::ErrModule);
        }
    };
    (*table)
        .set(&mut state.alloc, key_val, ev)
        .map_err(|_| ElaraStatus::ErrOom)?;
    *base.add(a as usize) = ev;
    Ok(())
}

fn byte_frame(state: &ElaraState, logical: &str) -> LoadFrame {
    LoadFrame {
        search_root: state.module_anchor.clone(),
        rel_dir: library::parent_key(logical),
    }
}

unsafe fn finish_module_load(
    state: &mut ElaraState,
    table: *mut ObjTable,
    spec_val: Value,
    base: *mut Value,
    a: u8,
    result: Result<Value, ElaraStatus>,
) -> Result<(), ElaraStatus> {
    match result {
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
