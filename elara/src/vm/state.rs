//! Minimal VM host state (gas + allocator + string pool) for the dispatch loop.

use core::ffi::c_void;

use crate::ffi::config::{ElaraModuleLoaderFn, state_for_tests};
use crate::gc::GcAllocator;
use crate::value::{GcHeader, InternPool, ObjTable, Value};
use crate::vm::error::ErrorState;
use crate::vm::fiber::ObjFiber;
use crate::vm::upvalue::ObjUpvalue;

/// Runtime state shared across fibers for a single embedder context.
pub struct ElaraState {
    pub alloc: GcAllocator,
    pub all_objects: *mut GcHeader,
    pub strings: InternPool,
    /// Incremental tri-color garbage collector control block.
    pub gc: crate::gc::GcState,
    /// Pre-allocated stack-trace / OOM error buffers.
    pub error: ErrorState,
    /// Fiber currently executing inside the VM (null when idle).
    pub current_fiber: *mut ObjFiber,
    /// Interned `"__index"` key for metatable lookup.
    pub mm_index: Value,
    /// Interned `"__newindex"` key for metatable lookup.
    pub mm_newindex: Value,
    /// Interned `"__add"` key for metamethod fallback.
    pub mm_add: Value,
    /// Interned `"__call"` key for callable tables.
    pub mm_call: Value,
    /// Interned `"__close"` key for `defer` resource cleanup.
    pub mm_close: Value,
    /// Interned `"__mode"` key for weak-table metatable mode.
    pub mm_mode: Value,
    /// Ordered log of defer / `__close` hooks run by `close_upvalues_from` (test/host aid).
    pub close_hook_log: [Value; 8],
    pub close_hook_log_len: usize,
    /// Last runtime error message (static) for host/test inspection.
    pub last_error: Option<&'static str>,
    /// When true, child-fiber errors are caught by `fiber.try_resume`.
    pub try_resume_mode: bool,
    /// Set by error unwinding when a `pcall` caught the error; dispatch reloads the caller.
    pub(crate) resume_after_catch: bool,
    pub gas_remaining: i64,
    pub gas_limit: i64,
    /// Sandbox capability bitmask (`ELARA_CAP_*`).
    pub capabilities: u32,
    /// Global environment table (may be null before open completes).
    pub globals: *mut ObjTable,
    /// Singleton module cache (`specifier` → exports table or loading sentinel).
    pub modules_table: *mut ObjTable,
    /// Optional host module loader.
    pub module_loader: Option<ElaraModuleLoaderFn>,
    pub module_loader_ud: *mut c_void,
    /// NUL-terminated dynamic error message buffer (module loader / cycle / not-found).
    pub error_buf: [u8; 256],
    pub error_buf_len: usize,
}

impl ElaraState {
    /// Creates a state with unlimited gas and full capabilities (test / internal default).
    #[must_use]
    pub fn new() -> Self {
        state_for_tests()
    }

    /// Creates a state with a hard instruction budget (full capabilities).
    #[must_use]
    pub fn with_gas(limit: i64) -> Self {
        let mut s = state_for_tests();
        s.gas_remaining = limit;
        s.gas_limit = limit;
        s
    }

    /// Records a static runtime error message and returns [`ElaraStatus::ErrRuntime`].
    pub fn runtime_error(&mut self, msg: &'static str) -> crate::vm::status::ElaraStatus {
        self.last_error = Some(msg);
        self.error.last_status = crate::vm::status::ElaraStatus::ErrRuntime;
        crate::vm::status::ElaraStatus::ErrRuntime
    }

    /// Forward write barrier for upvalue stores.
    #[inline]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn gc_barrier_upvalue(&mut self, upval: *mut ObjUpvalue, val: Value) {
        // SAFETY: callers pass a live upvalue from the open/closed upvalue paths.
        unsafe {
            crate::gc::barrier::barrier_upvalue(self, upval, val);
        }
    }

    /// Clears the defer / `__close` observation log.
    pub fn clear_close_hook_log(&mut self) {
        self.close_hook_log = [Value::NIL; 8];
        self.close_hook_log_len = 0;
    }

    pub(crate) fn push_close_hook(&mut self, v: Value) {
        if self.close_hook_log_len < self.close_hook_log.len() {
            self.close_hook_log[self.close_hook_log_len] = v;
            self.close_hook_log_len += 1;
        }
    }
}

impl Default for ElaraState {
    fn default() -> Self {
        Self::new()
    }
}
