//! Canonical VM / host status codes (`ElaraStatus`).

/// Result codes for compilation, execution, and host FFI.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElaraStatus {
    Ok = 0,
    Yield = 1,
    ErrSyntax = 2,
    ErrRuntime = 3,
    ErrOom = 4,
    ErrGas = 5,
    ErrVerify = 6,
    ErrModule = 7,
}
