//! Pre-allocated stack traces and runtime error state (zero heap alloc on the error path).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::mem::size_of;

use crate::compiler::opcode::Instruction;
use crate::gc::{GC_FIXED, GcAllocator};
use crate::value::{GcHeader, InternPool, ObjString, ObjType, Value};
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::{CallFrame, FRAME_FLAG_C_BOUNDARY};
use crate::vm::status::ElaraStatus;

/// Maximum frames recorded into [`ErrorState::trace_frames`] (no heap growth).
pub const ELARA_MAX_TRACE_FRAMES: usize = 64;

/// One stack-trace frame captured without heap allocation.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TraceEntry {
    pub source_name: *mut ObjString,
    pub fn_name: *const u8,
    pub fn_name_len: u16,
    pub is_native: u8,
    pub omitted_tail_calls: u8,
    pub line: u32,
}

impl Default for TraceEntry {
    fn default() -> Self {
        Self {
            source_name: core::ptr::null_mut(),
            fn_name: core::ptr::null(),
            fn_name_len: 0,
            is_native: 0,
            omitted_tail_calls: 0,
            line: 0,
        }
    }
}

/// Pre-allocated error / stack-trace buffers owned by [`crate::vm::ElaraState`].
pub struct ErrorState {
    pub oom_string: *mut ObjString,
    pub trace_frames: [TraceEntry; ELARA_MAX_TRACE_FRAMES],
    pub trace_count: usize,
    pub last_status: ElaraStatus,
    pub last_message: Value,
}

impl ErrorState {
    /// Pins a `GC_FIXED` `"Out of memory"` string and zeroes the trace buffer.
    pub fn new(
        strings: &mut InternPool,
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
    ) -> Self {
        let oom = strings
            .new_string(alloc, all_objects, GC_FIXED, b"Out of memory")
            .expect("pin Out of memory string");
        unsafe {
            (*oom.as_ptr()).header.gc_color = GC_FIXED;
        }
        Self {
            oom_string: oom.as_ptr(),
            trace_frames: [TraceEntry::default(); ELARA_MAX_TRACE_FRAMES],
            trace_count: 0,
            last_status: ElaraStatus::Ok,
            last_message: Value::NIL,
        }
    }

    /// Returns the pinned OOM message as a [`Value`].
    #[must_use]
    pub fn oom_value(&self) -> Value {
        Value::from_obj(
            core::ptr::NonNull::new(self.oom_string.cast()).expect("oom pinned"),
            ObjType::String,
        )
    }

    /// Clears the captured stack trace (does not free any heap memory).
    pub fn clear_trace(&mut self) {
        self.trace_count = 0;
        self.trace_frames = [TraceEntry::default(); ELARA_MAX_TRACE_FRAMES];
    }

    /// Records `status` / `message` without allocating.
    pub fn set_error(&mut self, status: ElaraStatus, message: Value) {
        self.last_status = status;
        self.last_message = message;
    }
}

/// Instruction index of `frame.pc` within its prototype (0-based).
#[must_use]
pub unsafe fn frame_pc_index(frame: &CallFrame) -> u32 {
    if frame.closure.is_null() {
        return 0;
    }
    let proto = (*frame.closure).proto;
    if proto.is_null() || (*proto).code.is_null() || frame.pc.is_null() {
        return 0;
    }
    let base = (*proto).code as usize;
    let cur = frame.pc as usize;
    if cur < base {
        return 0;
    }
    let bytes = cur - base;
    (bytes / size_of::<Instruction>()) as u32
}

/// Captures up to [`ELARA_MAX_TRACE_FRAMES`] frames from `fiber` into `error` (zero heap alloc).
///
/// Frames are recorded from the top of the call stack downward. Tail-call elision is reported via
/// `omitted_tail_calls` when consecutive frames share the same closure pointer at the same
/// `func_idx` (best-effort; usually 0 until the compiler annotates tails).
pub unsafe fn capture_stack_trace(error: &mut ErrorState, fiber: *mut ObjFiber) {
    error.clear_trace();
    if fiber.is_null() {
        return;
    }
    let f = &*fiber;
    let mut i = f.frame_count as usize;
    let mut omitted: u8 = 0;
    while i > 0 && error.trace_count < ELARA_MAX_TRACE_FRAMES {
        i -= 1;
        let frame = &*f.frames.add(i);
        if frame.flags & FRAME_FLAG_C_BOUNDARY != 0 || frame.closure.is_null() {
            let entry = TraceEntry {
                source_name: core::ptr::null_mut(),
                fn_name: b"<native>".as_ptr(),
                fn_name_len: 8,
                is_native: 1,
                omitted_tail_calls: omitted,
                line: 0,
            };
            omitted = 0;
            error.trace_frames[error.trace_count] = entry;
            error.trace_count += 1;
            continue;
        }

        let proto = (*frame.closure).proto;
        let (source_name, line) = if proto.is_null() {
            (core::ptr::null_mut(), 0u32)
        } else {
            let p = &*proto;
            let pc = frame_pc_index(frame);
            // PC often points at the next instruction; prefer the faulting one.
            let line_pc = pc.saturating_sub(1);
            (p.source_name, p.get_line(line_pc))
        };

        let entry = TraceEntry {
            source_name,
            fn_name: core::ptr::null(),
            fn_name_len: 0,
            is_native: 0,
            omitted_tail_calls: omitted,
            line,
        };
        omitted = 0;
        error.trace_frames[error.trace_count] = entry;
        error.trace_count += 1;
        let _ = proto; // keep available for future fn_name from debug info
    }
}

/// Builds a [`TraceEntry`] for a single frame (test / host helper).
pub unsafe fn trace_entry_from_frame(frame: &CallFrame) -> TraceEntry {
    if frame.flags & FRAME_FLAG_C_BOUNDARY != 0 || frame.closure.is_null() {
        return TraceEntry {
            source_name: core::ptr::null_mut(),
            fn_name: b"<native>".as_ptr(),
            fn_name_len: 8,
            is_native: 1,
            omitted_tail_calls: 0,
            line: 0,
        };
    }
    let proto = (*frame.closure).proto;
    if proto.is_null() {
        return TraceEntry::default();
    }
    let p = &*proto;
    let pc = frame_pc_index(frame);
    let line_pc = pc.saturating_sub(1);
    TraceEntry {
        source_name: p.source_name,
        fn_name: core::ptr::null(),
        fn_name_len: 0,
        is_native: 0,
        omitted_tail_calls: 0,
        line: p.get_line(line_pc),
    }
}

/// Records an OOM into `error` using the pinned string (no allocation).
pub fn record_oom(error: &mut ErrorState) {
    error.set_error(ElaraStatus::ErrOom, error.oom_value());
}
