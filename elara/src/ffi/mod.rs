//! Host FFI primitives: managed userdata, native functions, and 0-based call contexts.
//!
//! The public `extern "C"` `elara_*` surface lives in [`c_api`].

pub mod c_api;
pub mod config;
pub mod context;
pub mod native;
pub mod sandbox;
pub mod userdata;

pub use config::{ElaraConfig, ElaraModuleLoaderFn};
pub use context::CallContext;
pub use native::{ElaraNativeFn, ObjNativeFn};
pub use sandbox::{
    ELARA_CAP_ALL, ELARA_CAP_BIT, ELARA_CAP_FIBER, ELARA_CAP_MATH, ELARA_CAP_NONE,
    ELARA_CAP_SAFE_DEFAULT, ELARA_CAP_STDIO, ELARA_CAP_STRING, ELARA_CAP_SYS_ENV, ELARA_CAP_TABLE,
    ensure_module_allowed, has_capability, module_required_cap,
};
pub use userdata::{ElaraFinalizer, ElaraTraceFn, ElaraUserdataDesc, ObjUserdata};
