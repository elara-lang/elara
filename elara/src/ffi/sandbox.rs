//! Capability bitmasks and sandbox module gate checks.

use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub const ELARA_CAP_NONE: u32 = 0;
pub const ELARA_CAP_MATH: u32 = 1 << 0;
pub const ELARA_CAP_STRING: u32 = 1 << 1;
pub const ELARA_CAP_TABLE: u32 = 1 << 2;
pub const ELARA_CAP_BIT: u32 = 1 << 3;
pub const ELARA_CAP_FIBER: u32 = 1 << 4;
pub const ELARA_CAP_SYS_ENV: u32 = 1 << 5;
pub const ELARA_CAP_STDIO: u32 = 1 << 6;
pub const ELARA_CAP_NATIVE: u32 = 1 << 7;
pub const ELARA_CAP_SAFE_DEFAULT: u32 =
    ELARA_CAP_MATH | ELARA_CAP_STRING | ELARA_CAP_TABLE | ELARA_CAP_BIT | ELARA_CAP_FIBER;
pub const ELARA_CAP_ALL: u32 = 0xFFFF_FFFF;

pub const GAS_EXHAUSTED_MSG: &str = "Execution gas limit exhausted";

/// Returns the capability bit required to load `module`, if any.
#[must_use]
pub fn module_required_cap(module: &str) -> Option<u32> {
    match module {
        "math" => Some(ELARA_CAP_MATH),
        "string" => Some(ELARA_CAP_STRING),
        "table" => Some(ELARA_CAP_TABLE),
        "bit" => Some(ELARA_CAP_BIT),
        "fiber" => Some(ELARA_CAP_FIBER),
        "sys" | "sys.env" => Some(ELARA_CAP_SYS_ENV),
        "stdio" | "io" => Some(ELARA_CAP_STDIO),
        _ => None,
    }
}

/// Returns `true` when `state.capabilities` includes every bit in `cap`.
#[inline]
#[must_use]
pub fn has_capability(state: &ElaraState, cap: u32) -> bool {
    cap == 0 || (state.capabilities & cap) == cap
}

/// Gates stdlib / host module registration against the sandbox capability bitmask.
pub fn ensure_module_allowed(state: &mut ElaraState, module: &str) -> Result<(), ElaraStatus> {
    if let Some(cap) = module_required_cap(module)
        && !has_capability(state, cap)
    {
        state.last_error = Some("sandbox: module capability denied");
        state.error.last_status = ElaraStatus::ErrModule;
        return Err(ElaraStatus::ErrModule);
    }
    Ok(())
}
