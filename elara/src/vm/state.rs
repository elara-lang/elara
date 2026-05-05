//! Minimal VM host state (gas + allocator + string pool) for the dispatch loop.

use crate::gc::GcAllocator;
use crate::value::{GcHeader, InternPool, ObjType, Value};
use crate::vm::fiber::ObjFiber;
use crate::vm::upvalue::ObjUpvalue;

/// Runtime state shared across fibers for a single embedder context.
pub struct ElaraState {
    pub alloc: GcAllocator,
    pub all_objects: *mut GcHeader,
    pub strings: InternPool,
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
    /// Ordered log of defer / `__close` hooks run by `close_upvalues_from` (test/host aid).
    pub close_hook_log: [Value; 8],
    pub close_hook_log_len: usize,
    /// Last runtime error message (static) for host/test inspection.
    pub last_error: Option<&'static str>,
    /// When true, child-fiber errors are caught by `fiber.try_resume`.
    pub try_resume_mode: bool,
    pub gas_remaining: i64,
    pub gas_limit: i64,
}

impl ElaraState {
    /// Creates a state with unlimited gas (`gas_limit == 0` disables the fuel check).
    #[must_use]
    pub fn new() -> Self {
        Self::with_gas_inner(i64::MAX / 4, 0)
    }

    /// Creates a state with a hard instruction budget.
    #[must_use]
    pub fn with_gas(limit: i64) -> Self {
        Self::with_gas_inner(limit, limit)
    }

    fn with_gas_inner(gas_remaining: i64, gas_limit: i64) -> Self {
        let mut alloc = GcAllocator::new();
        let mut all_objects = core::ptr::null_mut();
        let mut strings = InternPool::new(0xC0FF_EE42);
        let mm_index = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__index");
        let mm_newindex = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__newindex");
        let mm_add = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__add");
        let mm_call = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__call");
        let mm_close = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__close");
        Self {
            alloc,
            all_objects,
            strings,
            current_fiber: core::ptr::null_mut(),
            mm_index,
            mm_newindex,
            mm_add,
            mm_call,
            mm_close,
            close_hook_log: [Value::NIL; 8],
            close_hook_log_len: 0,
            last_error: None,
            try_resume_mode: false,
            gas_remaining,
            gas_limit,
        }
    }

    /// Records a static runtime error message and returns [`ElaraStatus::ErrRuntime`].
    pub fn runtime_error(&mut self, msg: &'static str) -> crate::vm::status::ElaraStatus {
        self.last_error = Some(msg);
        crate::vm::status::ElaraStatus::ErrRuntime
    }

    /// Write barrier placeholder for closed upvalues (full GC lands later).
    #[inline]
    pub fn gc_barrier_upvalue(&mut self, _upval: *mut ObjUpvalue, _val: Value) {}

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
