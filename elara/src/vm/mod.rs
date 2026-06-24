//! Register-based virtual machine: call frames, fibers, and dispatch.

mod arith;
mod call;
pub mod closure;
mod crc32c;
pub mod debug;
pub mod dispatch;
pub mod error;
pub mod fiber;
pub mod frame;
mod meta;
pub mod module;
pub mod sched;
pub mod state;
pub mod status;
mod tab;
pub mod unwind;
pub mod upvalue;
pub mod verify;
mod verify_check;

pub use closure::ObjClosure;
pub use debug::{
    DebugState, ELARA_HOOK_CALL, ELARA_HOOK_COUNT, ELARA_HOOK_LINE, ELARA_HOOK_RET,
    ElaraDebugEvent, ElaraFrameInfo, ElaraHookFn, debug_get_frame, debug_get_local,
    debug_set_local, set_hook,
};
pub use dispatch::execute;
pub use error::{
    ELARA_MAX_TRACE_FRAMES, ErrorState, TraceEntry, capture_stack_trace, frame_pc_index,
    record_oom, trace_entry_from_frame,
};
pub use fiber::{
    ELARA_MAX_CALL_DEPTH, ELARA_MAX_STACK_SLOTS, ELARA_MIN_STACK_SLOTS, FiberStatus, ObjFiber,
};
pub use frame::{CallFrame, FRAME_FLAG_C_BOUNDARY, FRAME_FLAG_PCALL};
pub use module::{MODULE_LOADING_SENTINEL, op_import};
pub use sched::{
    FinishOutcome, TryResumeResult, YieldOutcome, fiber_create, fiber_resume, fiber_status,
    fiber_status_name, fiber_try_resume, fiber_value, fiber_yield, value_as_str,
};
pub use state::ElaraState;
pub use status::ElaraStatus;
pub use unwind::{
    CatchOutcome, intrinsic_assert, intrinsic_panic, push_pcall_frame, raise_runtime, unwind_fiber,
};
pub use upvalue::{
    ObjUpvalue, capture_upvalue, close_upvalues_from, fill_upvalues, rebase_open_upvalues,
};
pub use verify::{
    ELAC_ENDIAN_LE, ELAC_HEADER_SIZE, ELAC_MAGIC, ELAC_PTR_SIZE, ELAC_VERSION_MAJOR,
    ELAC_VERSION_MINOR, deserialize_elac, serialize_elac,
};
pub use verify_check::verify_proto;
