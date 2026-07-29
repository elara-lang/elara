//! Intrinsic standard library: core globals and capability-gated modules.

#![allow(clippy::missing_safety_doc)]

mod bit;
mod fiber;
mod globals;
mod math;
mod string;
mod sys;
mod table;
mod util;

use crate::value::nan::Value;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Registers core globals (`type`, `tostring`, …) on a freshly opened state.
pub unsafe fn open_stdlib(state: &mut ElaraState) -> Result<(), ElaraStatus> {
    unsafe { globals::register(state) }
}

/// Builds the exports table for an intrinsic module specifier, or `None` if unknown.
pub unsafe fn intrinsic_module(
    state: &mut ElaraState,
    spec: &str,
) -> Result<Option<Value>, ElaraStatus> {
    Ok(Some(match spec {
        "math" => unsafe { math::make_module(state)? },
        "bit" => unsafe { bit::make_module(state)? },
        "sys" => unsafe { sys::make_module(state)? },
        "string" => unsafe { string::make_module(state)? },
        "table" => unsafe { table::make_module(state)? },
        "fiber" => unsafe { fiber::make_module(state)? },
        _ => return Ok(None),
    }))
}
