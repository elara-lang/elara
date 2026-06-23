//! Host `elara_Config` layout and state construction helpers.
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::ffi::sandbox::{ELARA_CAP_ALL, ELARA_CAP_SAFE_DEFAULT};
use crate::gc::{ElaraAllocFn, GcAllocator, GcState, default_alloc_fn};
use crate::value::{GcHeader, InternPool, ObjType, Value};
use crate::vm::error::ErrorState;
use crate::vm::fiber::ObjFiber;
use crate::vm::state::ElaraState;
use crate::vm::upvalue::ObjUpvalue;

/// C-compatible open configuration (`elara_Config`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElaraConfig {
    pub alloc_fn: Option<ElaraAllocFn>,
    pub alloc_ud: *mut c_void,
    pub mem_limit_bytes: usize,
    pub gas_limit: u64,
    pub pause_budget_us: u32,
    pub capabilities: u32,
    pub hash_seed: u64,
}

impl ElaraConfig {
    /// Default sandbox-friendly host configuration.
    #[must_use]
    pub const fn default_config() -> Self {
        Self {
            alloc_fn: None,
            alloc_ud: core::ptr::null_mut(),
            mem_limit_bytes: 0,
            gas_limit: 0,
            pause_budget_us: 500,
            capabilities: ELARA_CAP_SAFE_DEFAULT,
            hash_seed: 0xC0FF_EE42,
        }
    }
}

/// Module loader callback matching the C `elara_ModuleLoaderFn` signature.
pub type ElaraModuleLoaderFn = unsafe extern "C" fn(
    state: *mut ElaraState,
    specifier: *const core::ffi::c_char,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
    out_is_bytecode: *mut bool,
    ud: *mut c_void,
) -> crate::vm::status::ElaraStatus;

/// Builds a new [`ElaraState`] from `cfg` (null uses [`ElaraConfig::default_config`]).
pub fn state_from_config(cfg: Option<&ElaraConfig>) -> ElaraState {
    let cfg = cfg.copied().unwrap_or_else(ElaraConfig::default_config);
    let alloc_fn = cfg.alloc_fn.unwrap_or(default_alloc_fn);
    let mut alloc = GcAllocator::with_custom(alloc_fn, cfg.alloc_ud, cfg.mem_limit_bytes);
    let mut all_objects = core::ptr::null_mut();
    let seed = (cfg.hash_seed as u32) ^ ((cfg.hash_seed >> 32) as u32);
    let mut strings = InternPool::new(if seed == 0 { 0xC0FF_EE42 } else { seed });

    let mm_index = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__index");
    let mm_newindex = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__newindex");
    let mm_add = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__add");
    let mm_call = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__call");
    let mm_close = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__close");
    let mm_mode = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__mode");
    mark_fixed(mm_index);
    mark_fixed(mm_newindex);
    mark_fixed(mm_add);
    mark_fixed(mm_call);
    mark_fixed(mm_close);
    mark_fixed(mm_mode);

    // Globals table is allocated lazily on first `elara_set_global` / `ensure_globals`.
    let globals = core::ptr::null_mut();

    let mut gc = GcState::new();
    gc.alloc_fn = alloc_fn;
    gc.alloc_ud = cfg.alloc_ud;
    gc.mem_limit_bytes = cfg.mem_limit_bytes;
    gc.pause_budget_us = if cfg.pause_budget_us == 0 {
        500
    } else {
        cfg.pause_budget_us
    };
    gc.all_objects = all_objects;
    gc.bytes_allocated = alloc.bytes_allocated;

    let error = ErrorState::new(&mut strings, &mut alloc, &mut all_objects);
    gc.all_objects = all_objects;
    gc.bytes_allocated = alloc.bytes_allocated;

    let (gas_remaining, gas_limit) = if cfg.gas_limit == 0 {
        (i64::MAX / 4, 0)
    } else {
        let g = cfg.gas_limit.min(i64::MAX as u64) as i64;
        (g, g)
    };

    ElaraState {
        alloc,
        all_objects,
        strings,
        gc,
        error,
        current_fiber: core::ptr::null_mut(),
        mm_index,
        mm_newindex,
        mm_add,
        mm_call,
        mm_close,
        mm_mode,
        close_hook_log: [Value::NIL; 8],
        close_hook_log_len: 0,
        last_error: None,
        try_resume_mode: false,
        resume_after_catch: false,
        gas_remaining,
        gas_limit,
        capabilities: if cfg.capabilities == 0 {
            // Explicit CAP_NONE stays none; open() default uses SAFE_DEFAULT via config.
            0
        } else {
            cfg.capabilities
        },
        globals,
        modules_table: core::ptr::null_mut(),
        module_loader: None,
        module_loader_ud: core::ptr::null_mut(),
        error_buf: [0u8; 256],
        error_buf_len: 0,
    }
}

/// Internal unlimited-gas, full-capability state used by unit tests (`ElaraState::new`).
pub fn state_for_tests() -> ElaraState {
    let mut cfg = ElaraConfig::default_config();
    cfg.capabilities = ELARA_CAP_ALL;
    cfg.gas_limit = 0;
    state_from_config(Some(&cfg))
}

fn intern_mm(
    strings: &mut InternPool,
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    bytes: &[u8],
) -> Value {
    let s = strings
        .new_string(alloc, all_objects, 0, bytes)
        .expect("intern metamethod name");
    Value::from_obj(s.cast(), ObjType::String)
}

fn mark_fixed(v: Value) {
    if v.is_obj() {
        unsafe {
            (*v.as_obj()).gc_color = crate::gc::GC_FIXED;
        }
    }
}

/// Tears down a state previously returned by `elara_open*`.
pub unsafe fn destroy_state(state: *mut ElaraState) {
    if state.is_null() {
        return;
    }
    let mut s = Box::from_raw(state);
    s.current_fiber = core::ptr::null_mut::<ObjFiber>();
    s.globals = core::ptr::null_mut();
    s.modules_table = core::ptr::null_mut();
    for _ in 0..32 {
        crate::gc::gc_collect(&mut s);
        if s.all_objects.is_null() {
            break;
        }
    }
    // Force-free any remaining linked objects (including GC_FIXED headers on the list).
    while !s.all_objects.is_null() {
        let obj = s.all_objects;
        s.all_objects = (*obj).next;
        crate::gc::sweep::free_object(&mut s, obj);
    }
    s.strings.free_all(&mut s.alloc);
    let _ = core::mem::size_of::<ObjUpvalue>();
}
