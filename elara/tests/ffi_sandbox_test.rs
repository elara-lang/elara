#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicUsize, Ordering};

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::ffi::{
    CallContext, ELARA_CAP_ALL, ELARA_CAP_SAFE_DEFAULT, ELARA_CAP_STDIO, ElaraConfig,
    ElaraUserdataDesc, ObjNativeFn, ObjUserdata, ensure_module_allowed,
};
use elara::gc::gc_collect;
use elara::value::{ObjType, Value};
use elara::vm::{CallFrame, ElaraState, ElaraStatus, ObjClosure, ObjFiber, execute};

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

unsafe extern "C" fn host_add(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    use elara::ffi::c_api::{elara_arg, elara_argc, elara_return, elara_to_int};
    assert_eq!(elara_argc(ctx), 2);
    let a = elara_to_int(elara_arg(ctx, 0));
    let b = elara_to_int(elara_arg(ctx, 1));
    elara_return(ctx, elara::ffi::c_api::elara_val_i48(a + b));
    let _ = state;
    ElaraStatus::Ok
}

#[test]
fn c_ffi_opens_state_registers_native_fn_and_exchanges_0_based_args_and_returns() {
    unsafe {
        use elara::ffi::c_api::*;
        let s = elara_open();
        assert!(!s.is_null());

        let native = elara_val_native_fn(s, host_add, core::ptr::null_mut());
        assert!(!elara_is_nil(native));

        let args = [elara_val_i48(20), elara_val_i48(22)];
        let mut results = [0u64; 2];
        let mut nret = 0u16;
        let st = elara_call(
            s,
            native,
            args.as_ptr(),
            2,
            results.as_mut_ptr(),
            2,
            &mut nret,
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(nret, 1);
        assert_eq!(elara_to_int(results[0]), 42);

        elara_close(s);
    }
}

fn emit(proto: &mut ObjProto, state: &mut ElaraState, inst: Instruction, line: u32) {
    proto
        .emit_instruction(&mut state.alloc, inst, line)
        .expect("emit");
}

#[test]
fn infinite_while_and_tailcall_loops_terminate_deterministically_with_err_gas() {
    // --- while / backward JMP ---
    unsafe {
        use elara::ffi::c_api::*;
        let mut cfg = ElaraConfig::default_config();
        cfg.gas_limit = 32;
        cfg.capabilities = ELARA_CAP_ALL;
        let s = elara_open_with_config(&cfg);
        assert!(!s.is_null());
        let state = &mut *s;

        let mut body = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("body");
        {
            let p = body.as_mut();
            p.max_stack_size = 2;
            emit(p, state, Instruction::make_sj(OpCode::Jmp, -1), 1);
        }
        let closure =
            ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, body.as_ptr()).expect("c");
        let mut fiber =
            ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 4).expect("f");
        let f = fiber.as_mut();
        *f.stack = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        f.open_top = 1;
        f.push_frame(
            &mut state.alloc,
            CallFrame::new(closure.as_ptr(), 1, 0, 0xFFFF),
        )
        .unwrap();

        let st = execute(state, fiber.as_ptr());
        assert_eq!(st, ElaraStatus::ErrGas);
        let err = elara_last_error(s);
        assert!(!err.is_null());
        let msg = core::ffi::CStr::from_ptr(err).to_str().unwrap();
        assert_eq!(msg, "Execution gas limit exhausted");
        let _ = (f, closure, body);
        elara_close(s);
    }

    // --- tail-call self loop ---
    unsafe {
        use elara::ffi::c_api::*;
        let mut cfg = ElaraConfig::default_config();
        cfg.gas_limit = 64;
        cfg.capabilities = ELARA_CAP_ALL;
        let s = elara_open_with_config(&cfg);
        let state = &mut *s;

        let mut body = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("body");
        {
            let p = body.as_mut();
            p.max_stack_size = 4;
            p.num_params = 0;
            // R[0] = closure; TailCall A=0 B=1 (no args)
            emit(
                p,
                state,
                Instruction::make_abc(OpCode::TailCall, 0, 1, 0),
                1,
            );
        }
        let closure =
            ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, body.as_ptr()).expect("c");
        let mut fiber =
            ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 4).expect("f");
        let f = fiber.as_mut();
        let cv = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        *f.stack = cv;
        *f.stack.add(1) = cv; // R[0] callable for TailCall
        f.open_top = 2;
        f.push_frame(
            &mut state.alloc,
            CallFrame::new(closure.as_ptr(), 1, 0, 0xFFFF),
        )
        .unwrap();

        let st = execute(state, fiber.as_ptr());
        assert_eq!(st, ElaraStatus::ErrGas);
        let err = elara_last_error(s);
        assert!(!err.is_null());
        assert_eq!(
            core::ffi::CStr::from_ptr(err).to_str().unwrap(),
            "Execution gas limit exhausted"
        );
        let _ = (f, closure, body);
        elara_close(s);
    }
}

#[test]
fn capability_bitmask_blocks_disabled_stdlib_modules_in_sandbox_mode() {
    unsafe {
        use elara::ffi::c_api::*;
        let cfg = ElaraConfig::default_config();
        assert_eq!(cfg.capabilities, ELARA_CAP_SAFE_DEFAULT);
        assert_eq!(cfg.capabilities & ELARA_CAP_STDIO, 0);

        let s = elara_open_with_config(&cfg);
        let state = &mut *s;

        assert!(ensure_module_allowed(state, "math").is_ok());
        assert!(ensure_module_allowed(state, "stdio").is_err());
        assert_eq!(
            core::ffi::CStr::from_ptr(elara_last_error(s))
                .to_str()
                .unwrap(),
            "sandbox: module capability denied"
        );

        // set_global of a gated module name is blocked.
        elara_set_global(s, c"stdio".as_ptr(), elara_val_i48(1));
        assert!(elara_is_nil(elara_get_global(s, c"stdio".as_ptr())));

        // With CAP_ALL, stdio is allowed.
        state.capabilities = ELARA_CAP_ALL;
        state.last_error = None;
        state.error.last_status = ElaraStatus::Ok;
        assert!(ensure_module_allowed(state, "stdio").is_ok());
        elara_set_global(s, c"stdio".as_ptr(), elara_val_i48(7));
        assert_eq!(elara_to_int(elara_get_global(s, c"stdio".as_ptr())), 7);

        elara_close(s);
    }
}
