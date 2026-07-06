//! `#[unsafe(no_mangle)] pub extern "C"` Host ABI matching `elara/include/elara.h`.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_void};
use core::mem::size_of;
use core::ptr::NonNull;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::ffi::config::{ElaraConfig, ElaraModuleLoaderFn, destroy_state, state_from_config};
use crate::ffi::context::CallContext;
use crate::ffi::native::{ElaraNativeFn, ObjNativeFn};
use crate::ffi::sandbox::{ensure_module_allowed, module_required_cap};
use crate::ffi::userdata::{ElaraUserdataDesc, ObjUserdata};
use crate::gc::{gc_collect, gc_step_with_budget};
use crate::value::nan::{ObjBoxedI64, ObjType, Value};
use crate::value::string::ObjString;
use crate::vm::closure::ObjClosure;
use crate::vm::dispatch::execute;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

fn catch_status(f: impl FnOnce() -> ElaraStatus) -> ElaraStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(s) => s,
        Err(_) => ElaraStatus::ErrRuntime,
    }
}

fn catch_ptr<T>(f: impl FnOnce() -> *mut T) -> *mut T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(p) => p,
        Err(_) => core::ptr::null_mut(),
    }
}

/* ---- Lifecycle ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_default_config() -> ElaraConfig {
    ElaraConfig::default_config()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_open() -> *mut ElaraState {
    catch_ptr(|| {
        let mut s = state_from_config(None);
        let _ = crate::stdlib::open_stdlib(&mut s);
        Box::into_raw(Box::new(s))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_open_with_config(cfg: *const ElaraConfig) -> *mut ElaraState {
    catch_ptr(|| {
        let c = if cfg.is_null() { None } else { Some(&*cfg) };
        let mut s = state_from_config(c);
        let _ = crate::stdlib::open_stdlib(&mut s);
        Box::into_raw(Box::new(s))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_close(state: *mut ElaraState) {
    let _ = catch_unwind(AssertUnwindSafe(|| destroy_state(state)));
}

/* ---- Sandbox quotas ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_set_gas_limit(state: *mut ElaraState, gas: u64) {
    if state.is_null() {
        return;
    }
    let s = &mut *state;
    if gas == 0 {
        s.gas_limit = 0;
        s.gas_remaining = i64::MAX / 4;
    } else {
        let g = gas.min(i64::MAX as u64) as i64;
        s.gas_limit = g;
        s.gas_remaining = g;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_get_gas_remaining(state: *const ElaraState) -> u64 {
    if state.is_null() {
        return 0;
    }
    (*state).gas_remaining.max(0) as u64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_set_mem_limit(state: *mut ElaraState, max_bytes: usize) {
    if state.is_null() {
        return;
    }
    (*state).alloc.mem_limit_bytes = max_bytes;
    (*state).gc.mem_limit_bytes = max_bytes;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_get_bytes_allocated(state: *const ElaraState) -> usize {
    if state.is_null() {
        return 0;
    }
    (*state).alloc.bytes_allocated
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_gc_collect(state: *mut ElaraState) {
    if state.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| gc_collect(&mut *state)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_gc_step(state: *mut ElaraState, budget_us: u32) {
    if state.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if budget_us != 0 {
            (*state).gc.pause_budget_us = budget_us;
        }
        let _ = gc_step_with_budget(&mut *state);
    }));
}

/* ---- Compile / exec (bytecode format deferred) ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_eval(
    state: *mut ElaraState,
    _name: *const c_char,
    _source: *const c_char,
    out_result: *mut u64,
) -> ElaraStatus {
    catch_status(|| {
        if state.is_null() {
            return ElaraStatus::ErrRuntime;
        }
        (*state).last_error = Some("source evaluation requires the bytecode pipeline");
        if !out_result.is_null() {
            *out_result = Value::NIL.0;
        }
        ElaraStatus::ErrSyntax
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_compile_to_bytecode(
    state: *mut ElaraState,
    _name: *const c_char,
    _source: *const c_char,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> ElaraStatus {
    catch_status(|| {
        if !out_buf.is_null() {
            *out_buf = core::ptr::null_mut();
        }
        if !out_len.is_null() {
            *out_len = 0;
        }
        if !state.is_null() {
            (*state).last_error = Some("bytecode serialization not available");
        }
        ElaraStatus::ErrVerify
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_free_buffer(state: *mut ElaraState, buf: *mut u8, len: usize) {
    if state.is_null() || buf.is_null() || len == 0 {
        return;
    }
    (*state).alloc.free_bytes(buf, len);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_load_bytecode(
    state: *mut ElaraState,
    buf: *const u8,
    len: usize,
    out_closure: *mut u64,
) -> ElaraStatus {
    catch_status(|| {
        if !out_closure.is_null() {
            *out_closure = Value::NIL.0;
        }
        if state.is_null() || buf.is_null() {
            return ElaraStatus::ErrVerify;
        }
        let s = &mut *state;
        let bytes = core::slice::from_raw_parts(buf, len);
        match crate::vm::verify::deserialize_elac(s, bytes) {
            Ok(proto) => {
                let closure =
                    match crate::vm::ObjClosure::new(&mut s.alloc, &mut s.all_objects, 0, proto) {
                        Some(c) => c,
                        None => return ElaraStatus::ErrOom,
                    };
                if !out_closure.is_null() {
                    *out_closure = Value::from_obj(closure.cast(), ObjType::Closure).0;
                }
                ElaraStatus::Ok
            }
            Err(e) => e,
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_call(
    state: *mut ElaraState,
    callable: u64,
    args: *const u64,
    num_args: u16,
    results: *mut u64,
    max_results: u16,
    out_num_results: *mut u16,
) -> ElaraStatus {
    catch_status(|| {
        if state.is_null() {
            return ElaraStatus::ErrRuntime;
        }
        let s = &mut *state;
        let callable = Value(callable);
        let arg_slice = if args.is_null() || num_args == 0 {
            &[][..]
        } else {
            core::slice::from_raw_parts(args.cast::<Value>(), num_args as usize)
        };
        let mut scratch = [Value::NIL; 16];
        let max_n = (max_results as usize).min(scratch.len());
        match call_value(s, callable, arg_slice, &mut scratch[..max_n]) {
            Ok(n) => {
                if !results.is_null() {
                    for (i, v) in scratch.iter().enumerate().take(n as usize) {
                        *results.add(i) = v.0;
                    }
                }
                if !out_num_results.is_null() {
                    *out_num_results = n;
                }
                ElaraStatus::Ok
            }
            Err(e) => e,
        }
    })
}

/// Invokes a closure or native with `args`, writing returns into `results`.
pub(crate) unsafe fn call_value(
    state: &mut ElaraState,
    callable: Value,
    args: &[Value],
    results: &mut [Value],
) -> Result<u16, ElaraStatus> {
    if callable.is_obj_type(ObjType::NativeFn) {
        return (*callable.as_obj().cast::<ObjNativeFn>()).invoke(state, args, results);
    }
    if !callable.is_obj_type(ObjType::Closure) {
        state.last_error = Some("elara_call: value is not callable");
        return Err(ElaraStatus::ErrRuntime);
    }
    let closure = callable.as_obj().cast::<ObjClosure>();
    let proto = &*(*closure).proto;
    let stack_slots = (1 + args.len() + proto.max_stack_size as usize).max(16) as u32;
    let mut fiber = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, stack_slots, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    let f = fiber.as_mut();
    *f.stack = callable;
    for (i, a) in args.iter().enumerate() {
        *f.stack.add(1 + i) = *a;
    }
    f.open_top = 1 + args.len() as u32;
    let max_res = if results.is_empty() {
        0xFFFF
    } else {
        results.len() as u16
    };
    let mut frame = CallFrame::new(closure, 1, 0, max_res);
    frame.vararg_count = args.len().saturating_sub(proto.num_params as usize) as u16;
    f.push_frame(&mut state.alloc, frame)?;
    state.current_fiber = fiber.as_ptr();
    let st = execute(state, fiber.as_ptr());
    state.current_fiber = core::ptr::null_mut();
    let n = if st == ElaraStatus::Ok {
        let nret = f.open_top.min(results.len() as u32) as u16;
        for (i, slot) in results.iter_mut().enumerate().take(nret as usize) {
            *slot = *f.stack.add(i);
        }
        Ok(nret)
    } else {
        Err(st)
    };
    let fiber_ptr = fiber.as_ptr();
    unlink_object(&mut state.all_objects, fiber_ptr.cast());
    (*fiber_ptr).free_fiber(&mut state.alloc);
    n
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

/* ---- CallContext ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_argc(ctx: *const CallContext) -> u16 {
    if ctx.is_null() {
        return 0;
    }
    (*ctx).argc()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_arg(ctx: *const CallContext, index: u16) -> u64 {
    if ctx.is_null() {
        return Value::NIL.0;
    }
    (*ctx).arg(index).0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_return(ctx: *mut CallContext, val: u64) {
    if ctx.is_null() {
        return;
    }
    (*ctx).return_one(Value(val));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_return_multi(ctx: *mut CallContext, vals: *const u64, count: u16) {
    if ctx.is_null() {
        return;
    }
    (*ctx).return_multi(vals.cast::<Value>(), count);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_context_userdata(ctx: *const CallContext) -> *mut c_void {
    if ctx.is_null() {
        return core::ptr::null_mut();
    }
    (*ctx).context_userdata()
}

/* ---- Values ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_nil() -> u64 {
    Value::NIL.0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_bool(b: bool) -> u64 {
    Value::from_bool(b).0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_int(state: *mut ElaraState, i: i64) -> u64 {
    if state.is_null() {
        return Value::NIL.0;
    }
    if Value::fits_i48(i) {
        return Value::from_i48(i).0;
    }
    let s = &mut *state;
    let raw = match s.alloc.alloc_bytes(size_of::<ObjBoxedI64>()) {
        Some(p) => p,
        None => return Value::NIL.0,
    };
    let ptr = raw.as_ptr().cast::<ObjBoxedI64>();
    core::ptr::write(ptr, ObjBoxedI64::new(i, 0));
    (*ptr).header.next = s.all_objects;
    s.all_objects = ptr.cast();
    Value::from_obj(NonNull::new(ptr.cast()).unwrap(), ObjType::BoxedI64).0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_i48(i: i64) -> u64 {
    if Value::fits_i48(i) {
        Value::from_i48(i).0
    } else {
        Value::NIL.0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_float(d: f64) -> u64 {
    Value::from_f64(d).0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_string(
    state: *mut ElaraState,
    str: *const c_char,
    len: usize,
) -> u64 {
    if state.is_null() || str.is_null() {
        return Value::NIL.0;
    }
    let s = &mut *state;
    let bytes = core::slice::from_raw_parts(str.cast::<u8>(), len);
    match s
        .strings
        .new_string(&mut s.alloc, &mut s.all_objects, 0, bytes)
    {
        Some(p) => Value::from_obj(p.cast(), ObjType::String).0,
        None => Value::NIL.0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_val_native_fn(
    state: *mut ElaraState,
    fn_: ElaraNativeFn,
    userdata: *mut c_void,
) -> u64 {
    if state.is_null() {
        return Value::NIL.0;
    }
    let s = &mut *state;
    match ObjNativeFn::new(&mut s.alloc, &mut s.all_objects, 0, fn_, userdata) {
        Some(p) => Value::from_obj(p.cast(), ObjType::NativeFn).0,
        None => Value::NIL.0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_new_userdata(
    state: *mut ElaraState,
    size: usize,
    desc: *const ElaraUserdataDesc,
    out_val: *mut u64,
) -> *mut c_void {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let s = &mut *state;
    match ObjUserdata::new(&mut s.alloc, &mut s.all_objects, 0, size, desc) {
        Some(p) => {
            if !out_val.is_null() {
                *out_val = Value::from_obj(p.cast(), ObjType::Userdata).0;
            }
            (*p.as_ptr()).payload_ptr().cast()
        }
        None => core::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_check_userdata(
    _state: *mut ElaraState,
    val: u64,
    expected_type_tag: u32,
) -> *mut c_void {
    ObjUserdata::check(Value(val), expected_type_tag).unwrap_or(core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_is_nil(v: u64) -> bool {
    Value(v).is_nil()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_is_int(v: u64) -> bool {
    Value(v).is_int()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_is_float(v: u64) -> bool {
    Value(v).is_float()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_is_string(v: u64) -> bool {
    Value(v).is_obj_type(ObjType::String)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_to_int(v: u64) -> i64 {
    Value(v).to_i64().unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_to_float(v: u64) -> f64 {
    let val = Value(v);
    if val.is_float() {
        val.as_f64()
    } else {
        val.to_i64().map(|i| i as f64).unwrap_or(0.0)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_to_string(
    _state: *mut ElaraState,
    v: u64,
    out_len: *mut usize,
) -> *const c_char {
    let val = Value(v);
    if !val.is_obj_type(ObjType::String) {
        if !out_len.is_null() {
            *out_len = 0;
        }
        return core::ptr::null();
    }
    let s = &*val.as_obj().cast::<ObjString>();
    if !out_len.is_null() {
        *out_len = s.byte_len as usize;
    }
    s.payload_ptr().cast()
}

/* ---- Globals / modules ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_set_module_loader(
    state: *mut ElaraState,
    loader: Option<ElaraModuleLoaderFn>,
    ud: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    (*state).module_loader = loader;
    (*state).module_loader_ud = ud;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_set_global(state: *mut ElaraState, name: *const c_char, val: u64) {
    if state.is_null() || name.is_null() {
        return;
    }
    let s = &mut *state;
    let cstr = core::ffi::CStr::from_ptr(name);
    let bytes = cstr.to_bytes();
    // Capability gate when registering known stdlib module tables by name.
    if let Ok(mod_name) = core::str::from_utf8(bytes)
        && module_required_cap(mod_name).is_some()
        && ensure_module_allowed(s, mod_name).is_err()
    {
        return;
    }
    if ensure_globals(s).is_none() {
        return;
    }
    let key = match s
        .strings
        .new_string(&mut s.alloc, &mut s.all_objects, 0, bytes)
    {
        Some(p) => Value::from_obj(p.cast(), ObjType::String),
        None => return,
    };
    let _ = (*s.globals).set(&mut s.alloc, key, Value(val));
}

unsafe fn ensure_globals(s: &mut ElaraState) -> Option<*mut crate::value::ObjTable> {
    if !s.globals.is_null() {
        return Some(s.globals);
    }
    let t = crate::value::ObjTable::new(&mut s.alloc, &mut s.all_objects, 0, 1, 0, 16)?;
    s.globals = t.as_ptr();
    Some(s.globals)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_get_global(state: *mut ElaraState, name: *const c_char) -> u64 {
    if state.is_null() || name.is_null() || (*state).globals.is_null() {
        return Value::NIL.0;
    }
    let s = &mut *state;
    let bytes = core::ffi::CStr::from_ptr(name).to_bytes();
    let key = match s
        .strings
        .new_string(&mut s.alloc, &mut s.all_objects, 0, bytes)
    {
        Some(p) => Value::from_obj(p.cast(), ObjType::String),
        None => return Value::NIL.0,
    };
    (*s.globals).get(key).0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_last_error(state: *const ElaraState) -> *const c_char {
    if state.is_null() {
        return core::ptr::null();
    }
    match (*state).last_error {
        Some("__dynamic__") if (*state).error_buf_len > 0 => (*state).error_buf.as_ptr().cast(),
        Some(msg) => c_error_ptr(msg),
        None => core::ptr::null(),
    }
}

fn c_error_ptr(msg: &str) -> *const c_char {
    // Host ABI requires a NUL-terminated C string; map known messages to `c"..."` literals.
    match msg {
        crate::ffi::sandbox::GAS_EXHAUSTED_MSG => c"Execution gas limit exhausted".as_ptr(),
        "sandbox: module capability denied" => c"sandbox: module capability denied".as_ptr(),
        "runtime error" => c"runtime error".as_ptr(),
        "source evaluation requires the bytecode pipeline" => {
            c"source evaluation requires the bytecode pipeline".as_ptr()
        }
        "bytecode serialization not available" => c"bytecode serialization not available".as_ptr(),
        "bytecode loading not available" => c"bytecode loading not available".as_ptr(),
        "elara_call: value is not callable" => c"elara_call: value is not callable".as_ptr(),
        "elac: truncated header" => c"elac: truncated header".as_ptr(),
        "elac: bad magic" => c"elac: bad magic".as_ptr(),
        "elac: unsupported version" => c"elac: unsupported version".as_ptr(),
        "elac: incompatible platform" => c"elac: incompatible platform".as_ptr(),
        "elac: truncated payload" => c"elac: truncated payload".as_ptr(),
        "elac: crc32c mismatch" => c"elac: crc32c mismatch".as_ptr(),
        "elac: empty code" => c"elac: empty code".as_ptr(),
        "elac: int48 out of range" => c"elac: int48 out of range".as_ptr(),
        "elac: bad const tag" => c"elac: bad const tag".as_ptr(),
        _ => c"error".as_ptr(),
    }
}

/* ---- Debug hooks ---- */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_set_hook(
    state: *mut ElaraState,
    hook: Option<crate::vm::debug::ElaraHookFn>,
    ud: *mut c_void,
    mask: u32,
    count_interval: u32,
) {
    if state.is_null() {
        return;
    }
    crate::vm::debug::set_hook(&mut *state, hook, ud, mask, count_interval);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_debug_get_frame(
    state: *const ElaraState,
    depth: u32,
    out_info: *mut crate::vm::debug::ElaraFrameInfo,
) -> bool {
    if state.is_null() {
        return false;
    }
    crate::vm::debug::debug_get_frame(&*state, depth, out_info)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_debug_get_local(
    state: *const ElaraState,
    depth: u32,
    reg_idx: u16,
    out_val: *mut u64,
) -> bool {
    if state.is_null() {
        return false;
    }
    crate::vm::debug::debug_get_local(&*state, depth, reg_idx, out_val)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn elara_debug_set_local(
    state: *mut ElaraState,
    depth: u32,
    reg_idx: u16,
    val: u64,
) -> bool {
    if state.is_null() {
        return false;
    }
    crate::vm::debug::debug_set_local(&mut *state, depth, reg_idx, val)
}
