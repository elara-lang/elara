//! Register-based virtual machine: call frames, fibers, and dispatch.

mod arith;
mod call;
pub mod closure;
pub mod dispatch;
pub mod fiber;
pub mod frame;
mod meta;
pub mod sched;
pub mod state;
pub mod status;
mod tab;
pub mod upvalue;

pub use closure::ObjClosure;
pub use dispatch::execute;
pub use fiber::{
    ELARA_MAX_CALL_DEPTH, ELARA_MAX_STACK_SLOTS, ELARA_MIN_STACK_SLOTS, FiberStatus, ObjFiber,
};
pub use frame::{CallFrame, FRAME_FLAG_C_BOUNDARY, FRAME_FLAG_PCALL};
pub use sched::{
    FinishOutcome, TryResumeResult, YieldOutcome, fiber_create, fiber_resume, fiber_status,
    fiber_status_name, fiber_try_resume, fiber_value, fiber_yield, value_as_str,
};
pub use state::ElaraState;
pub use status::ElaraStatus;
pub use upvalue::{
    ObjUpvalue, capture_upvalue, close_upvalues_from, fill_upvalues, rebase_open_upvalues,
};
