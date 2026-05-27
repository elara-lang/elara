#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicUsize, Ordering};

use elara::ffi::{CallContext, ElaraUserdataDesc, ObjNativeFn, ObjUserdata};
use elara::gc::gc_collect;
use elara::value::{ObjType, Value};
use elara::vm::{ElaraState, ElaraStatus};

static FINALIZER_HITS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn test_finalizer(
    _state: *mut ElaraState,
    data: *mut core::ffi::c_void,
    size: usize,
) {
    assert!(!data.is_null());
    assert_eq!(size, 8);
    // Payload was written as 0xDEAD_BEEF_u64 before becoming unreachable.
    let v = *(data as *const u64);
    assert_eq!(v, 0xDEAD_BEEF);
    FINALIZER_HITS.fetch_add(1, Ordering::SeqCst);
}

const TEST_UD_TAG: u32 = 0xA11C_E001;

static TEST_UD_DESC: ElaraUserdataDesc = ElaraUserdataDesc {
    type_name: c"test_ud".as_ptr(),
    type_tag: TEST_UD_TAG,
    finalize: Some(test_finalizer),
    trace: None,
};

#[test]
fn managed_userdata_verifies_type_tag_and_invokes_finalizer_on_gc_sweep() {
    FINALIZER_HITS.store(0, Ordering::SeqCst);
    let mut state = ElaraState::new();
    let baseline = state.alloc.bytes_allocated;

    let ud = ObjUserdata::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        8,
        &TEST_UD_DESC,
    )
    .expect("userdata");

    unsafe {
        let p = ud.as_ptr();
        *((*p).payload_ptr() as *mut u64) = 0xDEAD_BEEF;
        let val = Value::from_obj(
            core::ptr::NonNull::new(p.cast()).unwrap(),
            ObjType::Userdata,
        );

        let ok = ObjUserdata::check(val, TEST_UD_TAG).expect("tag match");
        assert_eq!(*(ok as *const u64), 0xDEAD_BEEF);
        assert!(ObjUserdata::check(val, TEST_UD_TAG ^ 1).is_none());
        assert!(ObjUserdata::check(Value::NIL, TEST_UD_TAG).is_none());

        // Drop the only root by not keeping `val` live; collect.
        let _ = val;
        gc_collect(&mut state);
    }

    assert_eq!(FINALIZER_HITS.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.alloc.bytes_allocated, baseline,
        "userdata payload must be reclaimed after finalizer"
    );

    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}

unsafe extern "C" fn add_native(_state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let c = &mut *ctx;
    let a = c.arg(0).to_i64().unwrap_or(0);
    let b = c.arg(1).to_i64().unwrap_or(0);
    assert_eq!(c.argc(), 2);
    assert!(c.context_userdata().is_null());
    c.return_one(Value::from_i48(a + b));
    ElaraStatus::Ok
}

#[test]
fn native_fn_call_context_exchanges_0_based_args_and_returns() {
    let mut state = ElaraState::new();
    let native = ObjNativeFn::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        add_native,
        core::ptr::null_mut(),
    )
    .expect("native");

    let args = [Value::from_i48(20), Value::from_i48(22)];
    let mut results = [Value::NIL; 2];
    unsafe {
        let n = (*native.as_ptr())
            .invoke(&mut state, &args, &mut results)
            .expect("invoke");
        assert_eq!(n, 1);
        assert_eq!(results[0].to_i64(), Some(42));

        ObjNativeFn::free(&mut state, native.as_ptr());
        // Unlink from all_objects so free_all / Drop of state paths stay clean.
        state.all_objects = core::ptr::null_mut();
    }
    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}
