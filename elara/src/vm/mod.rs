//! Register-based virtual machine: call frames, fibers, and dispatch.

mod arith;
pub mod closure;
pub mod dispatch;
pub mod fiber;
pub mod frame;
pub mod state;
pub mod status;

pub use closure::ObjClosure;
pub use dispatch::execute;
pub use fiber::ObjFiber;
pub use frame::{CallFrame, FRAME_FLAG_C_BOUNDARY, FRAME_FLAG_PCALL};
pub use state::ElaraState;
pub use status::ElaraStatus;
