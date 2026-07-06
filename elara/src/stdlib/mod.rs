//! Intrinsic standard library: core globals and capability-gated modules.

#![allow(clippy::missing_safety_doc)]

mod bit;
mod globals;
mod math;
mod sys;
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
        "string" | "table" | "fiber" => {
            // Bodies land in later tasks; return an empty capability-gated table.
            let t =
                crate::value::ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4)
                    .ok_or(ElaraStatus::ErrOom)?;
            Value::from_obj(
                core::ptr::NonNull::new(t.as_ptr().cast()).unwrap(),
                crate::value::ObjType::Table,
            )
        }
        _ => return Ok(None),
    }))
}
