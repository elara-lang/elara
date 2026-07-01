//! Debug hooks, live introspection, opcode profiler, and DAP stack snapshots.

mod hooks;
mod profile;

use core::ffi::c_void;

pub use hooks::{
    ELARA_HOOK_CALL, ELARA_HOOK_COUNT, ELARA_HOOK_LINE, ELARA_HOOK_RET, ElaraDebugEvent,
    ElaraFrameInfo, ElaraHookFn, debug_get_frame, debug_get_local, debug_set_local, invoke_hook,
    on_call, on_instruction, on_return,
};
pub use profile::{
    FnCounters, enable_profiler, format_dap_stack_trace, record_call, record_instruction,
    reset_profiler,
};

/// Per-state debug / hook / profiler control block.
pub struct DebugState {
    pub hook_fn: Option<ElaraHookFn>,
    pub hook_ud: *mut c_void,
    pub hook_mask: u32,
    pub count_interval: u32,
    pub count_remaining: u32,
    pub last_line: u32,
    pub in_hook: bool,
    /// When true, dispatch records opcode / function counters.
    pub profiling: bool,
    /// Histogram keyed by raw opcode byte (`0..63`).
    pub opcode_counts: [u64; 64],
    /// Per-prototype call and inclusive instruction counters.
    pub fn_counters: Vec<FnCounters>,
}

impl DebugState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            hook_fn: None,
            hook_ud: core::ptr::null_mut(),
            hook_mask: 0,
            count_interval: 0,
            count_remaining: 0,
            last_line: u32::MAX,
            in_hook: false,
            profiling: false,
            opcode_counts: [0; 64],
            fn_counters: Vec::new(),
        }
    }
}

impl Default for DebugState {
    fn default() -> Self {
        Self::new()
    }
}

/// Installs or clears the host debug hook.
pub fn set_hook(
    state: &mut crate::vm::state::ElaraState,
    hook: Option<ElaraHookFn>,
    ud: *mut c_void,
    mask: u32,
    count_interval: u32,
) {
    state.debug.hook_fn = hook;
    state.debug.hook_ud = ud;
    state.debug.hook_mask = mask;
    state.debug.count_interval = count_interval;
    state.debug.count_remaining = if mask & ELARA_HOOK_COUNT != 0 {
        count_interval.max(1)
    } else {
        0
    };
    state.debug.last_line = u32::MAX;
    state.debug.in_hook = false;
}
