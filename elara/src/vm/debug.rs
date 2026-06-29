//! Debug hooks (`CALL` / `RET` / `LINE` / `COUNT`) and live frame / local introspection.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_void};

use crate::compiler::ObjProto;
use crate::compiler::opcode::Instruction;
use crate::value::Value;
use crate::value::string::ObjString;
use crate::vm::error::frame_pc_index;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::{CallFrame, FRAME_FLAG_C_BOUNDARY};
use crate::vm::state::ElaraState;

pub const ELARA_HOOK_CALL: u32 = 1 << 0;
pub const ELARA_HOOK_RET: u32 = 1 << 1;
pub const ELARA_HOOK_LINE: u32 = 1 << 2;
pub const ELARA_HOOK_COUNT: u32 = 1 << 3;

/// Host debug hook callback (`elara_HookFn`).
pub type ElaraHookFn =
    unsafe extern "C" fn(state: *mut ElaraState, ev: *const ElaraDebugEvent, ud: *mut c_void);

/// Debug event payload delivered to [`ElaraHookFn`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElaraDebugEvent {
    pub event: u32,
    pub source_name: *const c_char,
    pub fn_name: *const c_char,
    pub line: u32,
    pub frame_depth: u32,
    pub pc_offset: u32,
}

/// Snapshot of one call frame for host introspection.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElaraFrameInfo {
    pub source_name: *const c_char,
    pub line: u32,
    pub num_params: u16,
    pub num_locals: u16,
    pub is_tail_call: u8,
    pub is_native: u8,
}

/// Per-state debug / hook control block.
pub struct DebugState {
    pub hook_fn: Option<ElaraHookFn>,
    pub hook_ud: *mut c_void,
    pub hook_mask: u32,
    pub count_interval: u32,
    pub count_remaining: u32,
    pub last_line: u32,
    pub in_hook: bool,
}

impl DebugState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hook_fn: None,
            hook_ud: core::ptr::null_mut(),
            hook_mask: 0,
            count_interval: 0,
            count_remaining: 0,
            last_line: u32::MAX,
            in_hook: false,
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
    state: &mut ElaraState,
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

#[inline]
unsafe fn cstr_from_obj_string(s: *mut ObjString) -> *const c_char {
    if s.is_null() {
        return core::ptr::null();
    }
    (*s).payload_ptr().cast()
}

#[inline]
unsafe fn proto_of(frame: &CallFrame) -> Option<&ObjProto> {
    if frame.closure.is_null() || frame.flags & FRAME_FLAG_C_BOUNDARY != 0 {
        return None;
    }
    let proto = (*frame.closure).proto;
    if proto.is_null() { None } else { Some(&*proto) }
}

unsafe fn fill_event_from_frame(frame: &CallFrame, event: u32, pc_offset: u32) -> ElaraDebugEvent {
    if let Some(p) = proto_of(frame) {
        ElaraDebugEvent {
            event,
            source_name: cstr_from_obj_string(p.source_name),
            fn_name: core::ptr::null(),
            line: p.get_line(pc_offset),
            frame_depth: 0,
            pc_offset,
        }
    } else {
        ElaraDebugEvent {
            event,
            source_name: core::ptr::null(),
            fn_name: c"<native>".as_ptr(),
            line: 0,
            frame_depth: 0,
            pc_offset: 0,
        }
    }
}

/// Invokes the host hook when `mask` includes `ev.event` and re-entry is not active.
pub unsafe fn invoke_hook(state: &mut ElaraState, ev: ElaraDebugEvent) {
    if state.debug.in_hook {
        return;
    }
    if state.debug.hook_mask & ev.event == 0 {
        return;
    }
    let Some(f) = state.debug.hook_fn else {
        return;
    };
    let ud = state.debug.hook_ud;
    state.debug.in_hook = true;
    f(state as *mut ElaraState, &ev, ud);
    state.debug.in_hook = false;
}

/// Per-instruction hook work (`COUNT` + `LINE`). No-op when `hook_mask == 0`.
#[inline]
pub unsafe fn on_instruction(state: &mut ElaraState, frame: &CallFrame, pc: *const Instruction) {
    let mask = state.debug.hook_mask;
    if mask == 0 {
        return;
    }

    let pc_offset = if let Some(p) = proto_of(frame) {
        if p.code.is_null() {
            0
        } else {
            pc.offset_from(p.code.cast_const()).wrapping_sub(1) as u32
        }
    } else {
        0
    };

    if mask & ELARA_HOOK_COUNT != 0 {
        if state.debug.count_remaining > 1 {
            state.debug.count_remaining -= 1;
        } else {
            state.debug.count_remaining = state.debug.count_interval.max(1);
            let mut ev = fill_event_from_frame(frame, ELARA_HOOK_COUNT, pc_offset);
            ev.frame_depth = 0;
            invoke_hook(state, ev);
        }
    }

    if mask & ELARA_HOOK_LINE != 0
        && let Some(p) = proto_of(frame)
    {
        let line = p.get_line(pc_offset);
        if line != 0 && line != state.debug.last_line {
            state.debug.last_line = line;
            let mut ev = fill_event_from_frame(frame, ELARA_HOOK_LINE, pc_offset);
            ev.line = line;
            invoke_hook(state, ev);
        }
    }
}

/// Fires `ELARA_HOOK_CALL` after a successful call / tail-call frame transition.
pub unsafe fn on_call(state: &mut ElaraState, frame: &CallFrame) {
    if state.debug.hook_mask & ELARA_HOOK_CALL == 0 {
        return;
    }
    state.debug.last_line = u32::MAX;
    let pc_offset = 0u32;
    invoke_hook(
        state,
        fill_event_from_frame(frame, ELARA_HOOK_CALL, pc_offset),
    );
}

/// Fires `ELARA_HOOK_RET` immediately before the active frame is popped.
pub unsafe fn on_return(state: &mut ElaraState, frame: &CallFrame) {
    if state.debug.hook_mask & ELARA_HOOK_RET == 0 {
        return;
    }
    let pc_offset = frame_pc_index(frame).saturating_sub(1);
    invoke_hook(
        state,
        fill_event_from_frame(frame, ELARA_HOOK_RET, pc_offset),
    );
}

unsafe fn frame_at(fiber: *const ObjFiber, depth: u32) -> Option<*mut CallFrame> {
    if fiber.is_null() {
        return None;
    }
    let f = &*fiber;
    if depth >= f.frame_count {
        return None;
    }
    let idx = f.frame_count - 1 - depth;
    Some(f.frames.add(idx as usize))
}

/// Fills `out` with info for the frame at `depth` (0 = active top). Returns `false` if missing.
pub unsafe fn debug_get_frame(state: &ElaraState, depth: u32, out: *mut ElaraFrameInfo) -> bool {
    if out.is_null() {
        return false;
    }
    let Some(fp) = frame_at(state.current_fiber, depth) else {
        return false;
    };
    let frame = &*fp;
    if let Some(p) = proto_of(frame) {
        let pc = frame_pc_index(frame);
        let line_pc = pc.saturating_sub(1);
        *out = ElaraFrameInfo {
            source_name: cstr_from_obj_string(p.source_name),
            line: p.get_line(line_pc),
            num_params: u16::from(p.num_params),
            num_locals: u16::from(p.max_stack_size),
            is_tail_call: 0,
            is_native: 0,
        };
    } else {
        *out = ElaraFrameInfo {
            source_name: core::ptr::null(),
            line: 0,
            num_params: 0,
            num_locals: 0,
            is_tail_call: 0,
            is_native: 1,
        };
    }
    true
}

/// Reads register `reg_idx` from the frame at `depth` into `out_val`.
pub unsafe fn debug_get_local(
    state: &ElaraState,
    depth: u32,
    reg_idx: u16,
    out_val: *mut u64,
) -> bool {
    if out_val.is_null() {
        return false;
    }
    let Some(fp) = frame_at(state.current_fiber, depth) else {
        return false;
    };
    let frame = &*fp;
    let Some(p) = proto_of(frame) else {
        return false;
    };
    if reg_idx as u32 >= u32::from(p.max_stack_size) {
        return false;
    }
    let fiber = &*state.current_fiber;
    let slot = fiber.stack.add(frame.base_idx as usize + reg_idx as usize);
    *out_val = (*slot).0;
    true
}

/// Writes `val` into register `reg_idx` of the frame at `depth`.
pub unsafe fn debug_set_local(state: &mut ElaraState, depth: u32, reg_idx: u16, val: u64) -> bool {
    let Some(fp) = frame_at(state.current_fiber, depth) else {
        return false;
    };
    let frame = &*fp;
    let Some(p) = proto_of(frame) else {
        return false;
    };
    if reg_idx as u32 >= u32::from(p.max_stack_size) {
        return false;
    }
    let fiber = &*state.current_fiber;
    let slot = fiber.stack.add(frame.base_idx as usize + reg_idx as usize);
    *slot = Value(val);
    true
}
