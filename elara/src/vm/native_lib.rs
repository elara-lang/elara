//! Host-registered native modules and `dlopen` of `elara_open_lib`.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use crate::value::nan::{ObjType, Value};
use crate::vm::library::{self, LoadFrame};
use crate::vm::module::set_module_error;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// `dlopen` mode: resolve symbols now, and keep them private to this object.
const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *mut c_char;
}

type SoOpenFn = unsafe extern "C" fn(*mut ElaraState, *mut u64) -> ElaraStatus;

/// Opener stored by `elara_register_native_module`.
pub type ElaraNativeModuleOpenFn =
    unsafe extern "C" fn(*mut ElaraState, *mut c_void, *mut u64) -> ElaraStatus;

/// One host opener. The VM does not free `ud`.
pub struct NativeModule {
    pub name: String,
    pub open: ElaraNativeModuleOpenFn,
    pub ud: *mut c_void,
}

/// Copies `name` with the import suffix strip and replaces a previous opener.
pub fn register(
    state: &mut ElaraState,
    name: &str,
    open: ElaraNativeModuleOpenFn,
    ud: *mut c_void,
) {
    let key = library::strip_module_suffix(name);
    if key.is_empty() {
        return;
    }
    if let Some(slot) = state.native_modules.iter_mut().find(|m| m.name == key) {
        slot.open = open;
        slot.ud = ud;
        return;
    }
    state.native_modules.push(NativeModule {
        name: key,
        open,
        ud,
    });
}

/// Runs a host opener when `key` was registered. `None` means fall through.
pub unsafe fn open_registered(
    state: &mut ElaraState,
    key: &str,
) -> Option<Result<Value, ElaraStatus>> {
    let (open, ud) = {
        let slot = state.native_modules.iter().find(|m| m.name == key)?;
        (slot.open, slot.ud)
    };
    let frame = LoadFrame {
        search_root: state.module_anchor.clone(),
        rel_dir: library::parent_key(key),
    };
    state.module_load_stack.push(frame);
    let mut out = Value::NIL.0;
    let before_len = state.error_buf_len;
    let before_status = state.error.last_status;
    let st = open(state, ud, &mut out);
    state.module_load_stack.pop();
    let kept_module_error = st != ElaraStatus::Ok
        && state.error.last_status == ElaraStatus::ErrModule
        && state.error_buf_len > 1
        && (state.error.last_status != before_status || state.error_buf_len != before_len);
    Some(accept_exports(
        state,
        key,
        st,
        Value(out),
        kept_module_error,
    ))
}

/// `dlopen`s `path` and calls `elara_open_lib`. A failure closes the handle immediately.
pub unsafe fn open_shared(
    state: &mut ElaraState,
    path: &str,
    key: &str,
) -> Result<Value, ElaraStatus> {
    let Ok(c_path) = std::ffi::CString::new(path) else {
        return fail_open(state, key);
    };
    let _ = dlerror();
    let handle = dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_LOCAL);
    if handle.is_null() {
        return fail_open(state, key);
    }
    let _ = dlerror();
    let sym = dlsym(handle, c"elara_open_lib".as_ptr());
    if sym.is_null() {
        let _ = dlclose(handle);
        return fail_open(state, key);
    }
    let open: SoOpenFn = core::mem::transmute(sym);
    let mut out = Value::NIL.0;
    let st = open(state, &mut out);
    if st != ElaraStatus::Ok || !Value(out).is_obj_type(ObjType::Table) {
        let _ = dlclose(handle);
        if st == ElaraStatus::Ok {
            set_module_error(
                state,
                format_args!("Module did not return an exports table"),
            );
            return Err(ElaraStatus::ErrModule);
        }
        return fail_open(state, key);
    }
    state.native_libs.push(handle as usize);
    Ok(Value(out))
}

/// Closes every library retained by a successful `dlopen`.
pub unsafe fn close_shared_libraries(state: &mut ElaraState) {
    for raw in state.native_libs.drain(..) {
        if raw != 0 {
            let _ = dlclose(raw as *mut c_void);
        }
    }
}

fn accept_exports(
    state: &mut ElaraState,
    key: &str,
    st: ElaraStatus,
    exports: Value,
    kept_module_error: bool,
) -> Result<Value, ElaraStatus> {
    if st != ElaraStatus::Ok {
        if !kept_module_error {
            return fail_open(state, key);
        }
        return Err(st);
    }
    if !exports.is_obj_type(ObjType::Table) {
        set_module_error(
            state,
            format_args!("Module did not return an exports table"),
        );
        return Err(ElaraStatus::ErrModule);
    }
    Ok(exports)
}

fn fail_open(state: &mut ElaraState, key: &str) -> Result<Value, ElaraStatus> {
    set_module_error(
        state,
        format_args!("Native library failed to open: '{key}'"),
    );
    Err(ElaraStatus::ErrModule)
}
