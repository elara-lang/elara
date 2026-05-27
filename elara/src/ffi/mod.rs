//! Host FFI primitives: managed userdata, native functions, and 0-based call contexts.
//!
//! The public `extern "C"` `elara_*` surface lives in `c_api` (added with the C99 header).

pub mod context;
pub mod native;
pub mod userdata;

pub use context::CallContext;
pub use native::{ElaraNativeFn, ObjNativeFn};
pub use userdata::{ElaraFinalizer, ElaraTraceFn, ElaraUserdataDesc, ObjUserdata};
