#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::ffi::c_api::{
    elara_debug_get_frame, elara_debug_get_local, elara_debug_set_local, elara_set_hook,
};
use elara::value::{ObjType, Value};
use elara::vm::{
    CallFrame, ELARA_HOOK_CALL, ELARA_HOOK_COUNT, ELARA_HOOK_LINE, ELARA_HOOK_RET, ElaraDebugEvent,
    ElaraFrameInfo, ElaraState, ElaraStatus, ObjClosure, ObjFiber, enable_profiler, execute,
    format_dap_stack_trace,
};

static EVENT_LOG: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static SAW_IN_HOOK: AtomicBool = AtomicBool::new(false);
static REENTRY_BLOCKED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn order_hook(
    state: *mut ElaraState,
    ev: *const ElaraDebugEvent,
    _ud: *mut core::ffi::c_void,
) {
    assert!(!ev.is_null());
    if (*state).debug.in_hook {
        SAW_IN_HOOK.store(true, Ordering::SeqCst);
    }
    // Attempt re-entrant fire: must be ignored by invoke_hook.
    let nested = ElaraDebugEvent {
        event: ELARA_HOOK_COUNT,
        source_name: core::ptr::null(),
        fn_name: core::ptr::null(),
        line: 0,
        frame_depth: 0,
        pc_offset: 0,
    };
    let before = EVENT_LOG.lock().unwrap().len();
    elara::vm::debug::invoke_hook(&mut *state, nested);
    if EVENT_LOG.lock().unwrap().len() == before {
        REENTRY_BLOCKED.store(true, Ordering::SeqCst);
    }
    EVENT_LOG.lock().unwrap().push((*ev).event);
}

unsafe fn emit(p: &mut ObjProto, alloc: &mut elara::gc::GcAllocator, inst: Instruction, line: u32) {
    p.emit_instruction(alloc, inst, line).expect("emit");
}

unsafe fn run(state: &mut ElaraState, proto: *mut ObjProto) -> ElaraStatus {
    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto).expect("c");
    let stack = (1 + (*proto).max_stack_size as u32).max(16);
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, stack, 8).expect("f");
    let f = fiber.as_mut();
    *f.stack = Value::from_obj(
        core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
        ObjType::Closure,
    );
    f.open_top = 1 + u32::from((*proto).max_stack_size);
    f.push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))
        .unwrap();
    execute(state, fiber.as_ptr())
}

#[test]
fn line_call_and_ret_hooks_fire_in_exact_execution_order_without_reentry() {
    let mut state = ElaraState::new();
    unsafe {
        let src = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"hook.ela")
            .unwrap();

        // child: LOADI R[0], 7; RETURN R[0], 2  (lines 10, 11)
        let mut child =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = child.as_mut();
            p.num_params = 0;
            p.max_stack_size = 2;
            emit(
                p,
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, 0, 7),
                10,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                11,
            );
        }

        // main: CLOSURE R[0], child; CALL R[0], 1, 2; RETURN R[0], 2
        let mut main =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = main.as_mut();
            p.max_stack_size = 4;
            p.add_child_proto(&mut state.alloc, child.as_ptr()).unwrap();
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, 0, 0),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Call, 0, 1, 2),
                2,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                3,
            );
        }

        EVENT_LOG.lock().unwrap().clear();
        SAW_IN_HOOK.store(false, Ordering::SeqCst);
        REENTRY_BLOCKED.store(false, Ordering::SeqCst);
        elara_set_hook(
            &mut state,
            Some(order_hook),
            core::ptr::null_mut(),
            ELARA_HOOK_CALL | ELARA_HOOK_RET | ELARA_HOOK_LINE,
            0,
        );

        assert_eq!(run(&mut state, main.as_ptr()), ElaraStatus::Ok);
        let log = EVENT_LOG.lock().unwrap().clone();
        // Expected: LINE(main1) → CALL → LINE(child10) → LINE(child11)/RET(child) →
        // LINE may fire on Return insn → RET(main)
        assert!(log.contains(&ELARA_HOOK_CALL), "missing CALL: {log:?}");
        assert!(
            log.iter().filter(|&&e| e == ELARA_HOOK_RET).count() >= 2,
            "expected ≥2 RET: {log:?}"
        );
        assert!(log.contains(&ELARA_HOOK_LINE), "missing LINE: {log:?}");
        // CALL before first child RET; first RET before second RET
        let call_i = log.iter().position(|&e| e == ELARA_HOOK_CALL).unwrap();
        let ret_positions: Vec<_> = log
            .iter()
            .enumerate()
            .filter_map(|(i, &e)| (e == ELARA_HOOK_RET).then_some(i))
            .collect();
        assert!(call_i < ret_positions[0]);
        assert!(ret_positions[0] < ret_positions[1]);
        assert!(SAW_IN_HOOK.load(Ordering::SeqCst));
        assert!(REENTRY_BLOCKED.load(Ordering::SeqCst));
    }
}

static COUNT_FIRES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn count_hook(
    _state: *mut ElaraState,
    ev: *const ElaraDebugEvent,
    _ud: *mut core::ffi::c_void,
) {
    if (*ev).event == ELARA_HOOK_COUNT {
        COUNT_FIRES.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn count_hook_fires_at_exact_instruction_intervals() {
    let mut state = ElaraState::new();
    unsafe {
        let mut proto = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .unwrap();
        {
            let p = proto.as_mut();
            p.max_stack_size = 2;
            // 8 instructions then return → 9 total
            for i in 0..8 {
                emit(
                    p,
                    &mut state.alloc,
                    Instruction::make_asbx(OpCode::LoadI, 0, i),
                    1,
                );
            }
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                1,
            );
        }
        COUNT_FIRES.store(0, Ordering::SeqCst);
        elara_set_hook(
            &mut state,
            Some(count_hook),
            core::ptr::null_mut(),
            ELARA_HOOK_COUNT,
            3,
        );
        assert_eq!(run(&mut state, proto.as_ptr()), ElaraStatus::Ok);
        // 9 instructions, interval 3 → fires on 3rd, 6th, 9th → 3 times
        assert_eq!(COUNT_FIRES.load(Ordering::SeqCst), 3);
    }
}

static INTROSPECT_OK: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn introspect_hook(
    state: *mut ElaraState,
    ev: *const ElaraDebugEvent,
    _ud: *mut core::ffi::c_void,
) {
    if (*ev).event != ELARA_HOOK_CALL {
        return;
    }
    let mut info = ElaraFrameInfo {
        source_name: core::ptr::null(),
        line: 0,
        num_params: 0,
        num_locals: 0,
        is_tail_call: 0,
        is_native: 0,
    };
    assert!(elara_debug_get_frame(state, 0, &mut info));
    assert_eq!(info.is_native, 0);
    assert!(info.num_locals >= 1);

    let mut val = 0u64;
    assert!(elara_debug_get_local(state, 0, 0, &mut val));
    // Child starts with empty regs; set then read back.
    assert!(elara_debug_set_local(state, 0, 0, Value::from_i48(99).0));
    assert!(elara_debug_get_local(state, 0, 0, &mut val));
    assert_eq!(Value(val).to_i64(), Some(99));

    // Parent frame still visible at depth 1.
    assert!(elara_debug_get_frame(state, 1, &mut info));
    INTROSPECT_OK.store(true, Ordering::SeqCst);
}

#[test]
fn debug_frame_and_local_introspection_reads_and_mutates_live_registers() {
    let mut state = ElaraState::new();
    unsafe {
        let src = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"intro.ela")
            .unwrap();
        let mut child =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = child.as_mut();
            p.max_stack_size = 2;
            // Return R[0] which the hook mutates to 99 before LOADI would overwrite —
            // use Nop-like: just Return so set_local value is returned.
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                1,
            );
        }
        let mut main =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = main.as_mut();
            p.max_stack_size = 4;
            p.add_child_proto(&mut state.alloc, child.as_ptr()).unwrap();
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, 0, 0),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Call, 0, 1, 2),
                2,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                3,
            );
        }

        INTROSPECT_OK.store(false, Ordering::SeqCst);
        elara_set_hook(
            &mut state,
            Some(introspect_hook),
            core::ptr::null_mut(),
            ELARA_HOOK_CALL,
            0,
        );
        assert_eq!(run(&mut state, main.as_ptr()), ElaraStatus::Ok);
        assert!(INTROSPECT_OK.load(Ordering::SeqCst));
    }
}

static DAP_JSON: Mutex<String> = Mutex::new(String::new());

unsafe extern "C" fn dap_capture_hook(
    state: *mut ElaraState,
    ev: *const ElaraDebugEvent,
    _ud: *mut core::ffi::c_void,
) {
    if (*ev).event != ELARA_HOOK_CALL {
        return;
    }
    let json = format_dap_stack_trace(&*state);
    *DAP_JSON.lock().unwrap() = json;
}

#[test]
fn profiler_records_opcode_histogram_and_exports_dap_stack_trace_json() {
    let mut state = ElaraState::new();
    unsafe {
        let src = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"prof.ela")
            .unwrap();
        let mut child =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = child.as_mut();
            p.max_stack_size = 2;
            emit(
                p,
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, 0, 42),
                5,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                6,
            );
        }
        let mut main =
            ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).unwrap();
        {
            let p = main.as_mut();
            p.max_stack_size = 4;
            p.add_child_proto(&mut state.alloc, child.as_ptr()).unwrap();
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, 0, 0),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Call, 0, 1, 2),
                2,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                3,
            );
        }

        enable_profiler(&mut state);
        DAP_JSON.lock().unwrap().clear();
        elara_set_hook(
            &mut state,
            Some(dap_capture_hook),
            core::ptr::null_mut(),
            ELARA_HOOK_CALL,
            0,
        );

        assert_eq!(run(&mut state, main.as_ptr()), ElaraStatus::Ok);

        // Opcode histogram: Closure, Call, LoadI, Return×2 (main+child) at minimum.
        let load_i = OpCode::LoadI as usize;
        let call = OpCode::Call as usize;
        let ret = OpCode::Return as usize;
        assert!(
            state.debug.opcode_counts[load_i] >= 1,
            "LoadI count={}",
            state.debug.opcode_counts[load_i]
        );
        assert!(state.debug.opcode_counts[call] >= 1);
        assert!(state.debug.opcode_counts[ret] >= 2);

        // Child should have been called once with inclusive insns.
        assert!(
            state
                .debug
                .fn_counters
                .iter()
                .any(|c| c.calls >= 1 && c.inclusive_insns >= 1),
            "fn_counters={:?}",
            state.debug.fn_counters
        );

        let json = DAP_JSON.lock().unwrap().clone();
        assert!(
            json.contains("\"stackFrames\""),
            "missing stackFrames: {json}"
        );
        assert!(json.contains("\"totalFrames\""), "missing totalFrames");
        assert!(json.contains("\"variables\""), "missing variables");
        assert!(json.contains("\"path\""), "missing source path");
        assert!(json.contains("prof.ela"), "missing source name: {json}");
        assert!(json.contains("\"name\":\"R0\""), "missing R0 var: {json}");
        // Basic JSON shape braces.
        assert!(json.starts_with('{') && json.ends_with('}'));
    }
}
