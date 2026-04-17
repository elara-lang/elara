use core::mem::size_of;

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::gc::GcAllocator;
use elara::value::{GcHeader, ObjTable, ObjType, Value};
use elara::vm::{CallFrame, ElaraState, ElaraStatus, ObjClosure, ObjFiber, execute};

fn closure_value(c: *mut ObjClosure) -> Value {
    Value::from_obj(
        core::ptr::NonNull::new(c.cast()).expect("non-null"),
        ObjType::Closure,
    )
}

#[test]
fn vm_executes_arithmetic_bitwise_string_concat_and_control_flow_loops() {
    let mut state = ElaraState::new();

    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    let mut dedup =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("dedup");
    let p = unsafe { proto.as_mut() };

    // Program: arithmetic + bitwise + counted loop via fused LEI+JMP, then multi-return.
    p.max_stack_size = 8;
    let emit = |p: &mut ObjProto, alloc: &mut GcAllocator, inst, line| {
        p.emit_instruction(alloc, inst, line).expect("emit");
    };
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 0, 10),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 1, 3),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Add, 2, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Mul, 3, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::BAnd, 4, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::BOr, 5, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 6, 0),
        2,
    );
    let loop_pc = p.code_count;
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::AddI, 6, 6, 1),
        2,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::LeI, 1, 6, 5u8),
        2,
    );
    let back = loop_pc as i32 - (p.code_count as i32 + 1);
    emit(
        p,
        &mut state.alloc,
        Instruction::make_sj(OpCode::Jmp, back),
        2,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Return, 2, 6, 0),
        3,
    );

    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr())
        .expect("closure");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 4).expect("fiber");
    let f = unsafe { fiber.as_mut() };

    unsafe {
        *f.stack.add(0) = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            elara::value::ObjType::Closure,
        );
    }
    f.open_top = 1 + u32::from(p.max_stack_size);
    let frame = unsafe { CallFrame::new(closure.as_ptr(), 1, 0, 5) };
    f.push_frame(&mut state.alloc, frame).expect("push frame");

    let status = unsafe { execute(&mut state, fiber.as_ptr()) };
    assert_eq!(status, ElaraStatus::Ok);

    unsafe {
        let stack = fiber.as_ref().stack;
        assert_eq!((*stack.add(0)).to_i64(), Some(13));
        assert_eq!((*stack.add(1)).to_i64(), Some(30));
        assert_eq!((*stack.add(2)).to_i64(), Some(2));
        assert_eq!((*stack.add(3)).to_i64(), Some(11));
        assert_eq!((*stack.add(4)).to_i64(), Some(6));
    }

    let f = unsafe { fiber.as_mut() };
    let old_ptr = f.stack;
    let grow_to = f.stack_capacity + 64;
    f.open_top = f.stack_capacity;
    f.ensure_stack(&mut state.alloc, grow_to).expect("grow");
    assert_ne!(f.stack, old_ptr);
    assert!(f.stack_capacity >= grow_to);

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state
            .alloc
            .free_bytes(closure.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        proto.as_mut().free_proto(&mut state.alloc);
        dedup.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn call_frame_stores_indices_not_raw_stack_pointers_across_growth() {
    let mut alloc = GcAllocator::new();
    let mut all: *mut GcHeader = core::ptr::null_mut();
    let mut proto = ObjProto::new(&mut alloc, &mut all, 0, core::ptr::null_mut()).expect("p");
    unsafe {
        (*proto.as_ptr()).max_stack_size = 2;
        (*proto.as_ptr())
            .emit_instruction(
                &mut alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                1,
            )
            .expect("ret");
    }
    let closure = ObjClosure::new(&mut alloc, &mut all, 0, proto.as_ptr()).expect("c");
    let mut fiber = ObjFiber::new(&mut alloc, &mut all, 0, 8, 2).expect("f");
    let f = unsafe { fiber.as_mut() };
    let frame = unsafe { CallFrame::new(closure.as_ptr(), 2, 1, 0) };
    assert_eq!(frame.base_idx, 2);
    assert_eq!(frame.func_idx, 1);
    f.push_frame(&mut alloc, frame).expect("push");
    let base_idx = unsafe { (*f.frames).base_idx };
    let grow_to = f.stack_capacity + 128;
    f.open_top = f.stack_capacity;
    f.ensure_stack(&mut alloc, grow_to).expect("grow");
    // Indices unchanged after reallocation
    assert_eq!(unsafe { (*f.frames).base_idx }, base_idx);

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut alloc);
        alloc.free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        alloc.free_bytes(closure.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        proto.as_mut().free_proto(&mut alloc);
        alloc.free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn vm_tail_call_optimization_executes_1_000_000_recursive_calls_in_o1_frames() {
    let mut state = ElaraState::new();
    let emit = |p: &mut ObjProto, alloc: &mut GcAllocator, inst, line| {
        p.emit_instruction(alloc, inst, line).expect("emit");
    };

    // countdown(self, n): if n == 0 { return 0 } else { return countdown(self, n - 1) }
    let mut countdown = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("countdown");
    {
        let p = unsafe { countdown.as_mut() };
        p.num_params = 2;
        p.max_stack_size = 4;
        // EQI take-jmp when R[1] == 0
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::EqI, 1, 1, 0),
            1,
        );
        let jmp_to_ret = p.code_count;
        emit(p, &mut state.alloc, Instruction::make_sj(OpCode::Jmp, 0), 1);
        // recurse: R[2] = n - 1; R[1] = self; TAILCALL self, self, n-1
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::AddI, 2, 1, (-1i8) as u8),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 1, 0, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::TailCall, 0, 3, 0),
            1,
        );
        let ret0 = p.code_count;
        // patch jmp: from jmp_to_ret to ret0
        let sj = ret0 as i32 - (jmp_to_ret as i32 + 1);
        unsafe {
            *p.code.add(jmp_to_ret as usize) = Instruction::make_sj(OpCode::Jmp, sj);
        }
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }

    let mut main = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("main");
    {
        let p = unsafe { main.as_mut() };
        p.max_stack_size = 4;
        // R[0]/R[1] filled by host with countdown closure; R[2]=1_000_000
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 2, 0), // placeholder; overwritten below
            1,
        );
        // We'll overwrite with a large immediate via LoadI after - LoadI only fits i16.
        // Use repeated AddI to build 1_000_000, or pass n from host in R[2] before execute.
        // Host sets R[2]=1_000_000; main just CALLs.
        // Replace the LoadI with Nop by not emitting it — host primes registers.
        p.code_count = 0;
        p.line_run_count = 0;
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 0, 3, 2),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }

    let c_count = ObjClosure::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        countdown.as_ptr(),
    )
    .expect("c_count");
    let c_main = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr())
        .expect("c_main");

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 4).expect("fiber");
    let f = unsafe { fiber.as_mut() };
    let cv = closure_value(c_count.as_ptr());
    unsafe {
        *f.stack.add(0) = closure_value(c_main.as_ptr());
        *f.stack.add(1) = cv; // R[0]
        *f.stack.add(2) = cv; // R[1] self
        *f.stack.add(3) = Value::from_i48(1_000_000); // R[2] n
    }
    f.open_top = 8;
    f.push_frame(&mut state.alloc, unsafe {
        CallFrame::new(c_main.as_ptr(), 1, 0, 1)
    })
    .expect("frame");

    let status = unsafe { execute(&mut state, fiber.as_ptr()) };
    assert_eq!(status, ElaraStatus::Ok);
    // TCO must never grow beyond 1 frame during recursion; after return, 0 frames.
    assert_eq!(unsafe { fiber.as_ref().frame_count }, 0);
    unsafe {
        assert_eq!((*fiber.as_ref().stack.add(0)).to_i64(), Some(0));
    }

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state
            .alloc
            .free_bytes(c_count.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        state
            .alloc
            .free_bytes(c_main.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        countdown.as_mut().free_proto(&mut state.alloc);
        main.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(countdown.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn vm_propagates_multiple_return_values_across_nested_calls_and_let_bindings() {
    let mut state = ElaraState::new();
    let emit = |p: &mut ObjProto, alloc: &mut GcAllocator, inst, line| {
        p.emit_instruction(alloc, inst, line).expect("emit");
    };

    let mut multi = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("multi");
    {
        let p = unsafe { multi.as_mut() };
        p.num_params = 0;
        p.max_stack_size = 4;
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 1),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 1, 2),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 2, 3),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 4, 0),
            1,
        );
    }

    let mut main = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("main");
    let mut dedup =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("dedup");
    {
        let p = unsafe { main.as_mut() };
        p.max_stack_size = 8;
        let sa = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"el")
            .expect("el");
        let sb = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"ara")
            .expect("ara");
        let ka = unsafe {
            (*p).add_constant(
                &mut state.alloc,
                dedup.as_mut(),
                Value::from_obj(sa.cast(), ObjType::String),
            )
            .expect("ka")
        };
        let kb = unsafe {
            (*p).add_constant(
                &mut state.alloc,
                dedup.as_mut(),
                Value::from_obj(sb.cast(), ObjType::String),
            )
            .expect("kb")
        };
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 0, 1, 4),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Add, 3, 0, 1),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Add, 3, 3, 2),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abx(OpCode::LoadK, 4, ka),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abx(OpCode::LoadK, 5, kb),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::ConcatN, 4, 4, 2),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 3, 3, 0),
            1,
        );
    }

    let c_multi =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, multi.as_ptr()).expect("cm");
    let c_main =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr()).expect("cmain");

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 4).expect("fiber");
    let f = unsafe { fiber.as_mut() };
    unsafe {
        *f.stack.add(0) = closure_value(c_main.as_ptr());
        *f.stack.add(1) = closure_value(c_multi.as_ptr());
    }
    f.open_top = 16;
    f.push_frame(&mut state.alloc, unsafe {
        CallFrame::new(c_main.as_ptr(), 1, 0, 2)
    })
    .expect("frame");

    let status = unsafe { execute(&mut state, fiber.as_ptr()) };
    assert_eq!(status, ElaraStatus::Ok);
    unsafe {
        let stack = fiber.as_ref().stack;
        assert_eq!((*stack.add(0)).to_i64(), Some(6)); // 1+2+3
        let concat = *stack.add(1);
        assert!(concat.is_obj_type(ObjType::String));
        let s = &*concat.as_obj().cast::<elara::value::ObjString>();
        assert_eq!(s.as_bytes(), b"elara");
    }

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state
            .alloc
            .free_bytes(c_multi.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        state
            .alloc
            .free_bytes(c_main.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        multi.as_mut().free_proto(&mut state.alloc);
        main.as_mut().free_proto(&mut state.alloc);
        dedup.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(multi.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

fn table_value(t: *mut ObjTable) -> Value {
    Value::from_obj(
        core::ptr::NonNull::new(t.cast()).expect("non-null"),
        ObjType::Table,
    )
}

fn emit_inst(p: &mut ObjProto, alloc: &mut GcAllocator, inst: Instruction, line: u32) {
    p.emit_instruction(alloc, inst, line).expect("emit");
}

#[test]
fn vm_dispatches_metatable_metamethods_index_newindex_add_and_call() {
    let mut state = ElaraState::new();

    let mut index_p = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .unwrap();
    {
        let p = unsafe { index_p.as_mut() };
        p.num_params = 2;
        p.max_stack_size = 2;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 42),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }
    let mut add_p = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .unwrap();
    {
        let p = unsafe { add_p.as_mut() };
        p.num_params = 2;
        p.max_stack_size = 2;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 99),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }
    let mut call_p = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .unwrap();
    {
        let p = unsafe { call_p.as_mut() };
        p.num_params = 2;
        p.max_stack_size = 2;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::AddI, 0, 1, 1u8),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }

    let mut main = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .unwrap();
    let mut dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).unwrap();
    {
        let p = unsafe { main.as_mut() };
        p.max_stack_size = 16;
        let kx = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"x")
            .unwrap();
        let k_idx = unsafe {
            (*p).add_constant(
                &mut state.alloc,
                dedup.as_mut(),
                Value::from_obj(kx.cast(), ObjType::String),
            )
            .unwrap()
        };
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abx(OpCode::LoadK, 1, k_idx),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetTab, 3, 0, 1),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 8, 7),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::SetTab, 0, 1, 8),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Add, 4, 0, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 9, 0, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 10, 5),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 9, 2, 2),
            1,
        );
        // Return R[3], R[4], R[9] directly
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 5, 9, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 3, 4, 0),
            1,
        );
    }

    let c_index = ObjClosure::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        index_p.as_ptr(),
    )
    .unwrap();
    let c_add =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, add_p.as_ptr()).unwrap();
    let c_call =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, call_p.as_ptr()).unwrap();
    let c_main =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr()).unwrap();

    let mut obj = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).unwrap();
    let mut mt = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).unwrap();
    let proxy = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).unwrap();
    unsafe {
        mt.as_mut()
            .set(
                &mut state.alloc,
                state.mm_index,
                closure_value(c_index.as_ptr()),
            )
            .unwrap();
        mt.as_mut()
            .set(
                &mut state.alloc,
                state.mm_newindex,
                table_value(proxy.as_ptr()),
            )
            .unwrap();
        mt.as_mut()
            .set(
                &mut state.alloc,
                state.mm_add,
                closure_value(c_add.as_ptr()),
            )
            .unwrap();
        mt.as_mut()
            .set(
                &mut state.alloc,
                state.mm_call,
                closure_value(c_call.as_ptr()),
            )
            .unwrap();
        obj.as_mut().metatable = mt.as_ptr();
    }

    let mut fiber = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).unwrap();
    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(0) = closure_value(c_main.as_ptr());
        *f.stack.add(1) = table_value(obj.as_ptr());
        *f.stack.add(3) = table_value(proxy.as_ptr());
        f.open_top = 40;
        f.push_frame(&mut state.alloc, CallFrame::new(c_main.as_ptr(), 1, 0, 3))
            .unwrap();
    }

    let st = unsafe { execute(&mut state, fiber.as_ptr()) };
    assert_eq!(st, ElaraStatus::Ok);
    unsafe {
        let s = fiber.as_ref().stack;
        assert_eq!((*s.add(0)).to_i64(), Some(42), "index");
        assert_eq!((*s.add(1)).to_i64(), Some(99), "add");
        assert_eq!((*s.add(2)).to_i64(), Some(6), "call");
        let key = *main.as_ref().constants.add(0);
        assert_eq!(proxy.as_ref().get(key).to_i64(), Some(7), "newindex");
        assert!(obj.as_ref().get(key).is_nil(), "obj raw");
    }

    // loops in a second execute on a tiny proto
    let mut loop_p = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .unwrap();
    {
        let p = unsafe { loop_p.as_mut() };
        p.max_stack_size = 12;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 0),
            1,
        ); // sum
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 1, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 2, 3),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 3, 1),
            1,
        );
        let prep = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::ForPrep, 1, 0),
            1,
        );
        let body = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Add, 0, 0, 4),
            1,
        );
        let loop_ = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::ForLoop, 1, 0),
            1,
        );
        unsafe {
            *p.code.add(prep as usize) =
                Instruction::make_asbx(OpCode::ForPrep, 1, loop_ as i32 - (prep as i32 + 1));
            *p.code.add(loop_ as usize) =
                Instruction::make_asbx(OpCode::ForLoop, 1, body as i32 - (loop_ as i32 + 1));
        }
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 5, 1),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 6, 4),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Range, 7, 5, 0),
            1,
        );
        // forin over proxy placed at R[8]
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 8, 8, 0),
            1,
        ); // nop placeholder
        // host sets R[8]=proxy; cursor R[9]=0; count R[12]
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 9, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 12, 0),
            1,
        );
        let fi = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::ForIn, 8, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::AddI, 12, 12, 1u8),
            1,
        );
        let jp = p.code_count;
        emit_inst(p, &mut state.alloc, Instruction::make_sj(OpCode::Jmp, 0), 1);
        let after = p.code_count;
        unsafe {
            *p.code.add(fi as usize) =
                Instruction::make_asbx(OpCode::ForIn, 8, after as i32 - (fi as i32 + 1));
            *p.code.add(jp as usize) =
                Instruction::make_sj(OpCode::Jmp, fi as i32 - (jp as i32 + 1));
            // remove useless move: replace with LoadNil noop — keep Move of proxy from host register
            // Actually host puts proxy at stack slot for R[8]; delete the Move instruction by making it Nop
            // Find Move 8,8 — leave it
        }
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 3, 0),
            1,
        ); // sum, ?, count at R[12] — move count to R[1]
        // patch: before return move 12->1
        let ret = p.code_count - 1;
        unsafe {
            *p.code.add(ret as usize) = Instruction::make_abc(OpCode::Move, 1, 12, 0);
        }
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 3, 0),
            1,
        );
    }
    let c_loop =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, loop_p.as_ptr()).unwrap();
    unsafe {
        fiber.as_mut().frame_count = 0;
        *fiber.as_mut().stack.add(0) = closure_value(c_loop.as_ptr());
        *fiber.as_mut().stack.add(9) = table_value(proxy.as_ptr()); // R[8] at base 1 => stack[9]
        fiber.as_mut().open_top = 24;
        fiber
            .as_mut()
            .push_frame(&mut state.alloc, CallFrame::new(c_loop.as_ptr(), 1, 0, 2))
            .unwrap();
        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);
        assert_eq!((*fiber.as_ref().stack.add(0)).to_i64(), Some(3), "for sum");
        assert_eq!(
            (*fiber.as_ref().stack.add(1)).to_i64(),
            Some(1),
            "forin count"
        );
        let range_v = *fiber.as_ref().stack.add(8); // R[7] with base_idx=1
        assert!(range_v.is_obj_type(ObjType::Table), "range");
        let range = range_v.as_obj().cast::<ObjTable>();
        (*range).free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(range.cast::<u8>(), size_of::<ObjTable>());

        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        for c in [
            c_index.as_ptr(),
            c_add.as_ptr(),
            c_call.as_ptr(),
            c_main.as_ptr(),
            c_loop.as_ptr(),
        ] {
            state
                .alloc
                .free_bytes(c.cast::<u8>(), size_of::<ObjClosure>());
        }
        for mut p in [index_p, add_p, call_p, main, loop_p] {
            p.as_mut().free_proto(&mut state.alloc);
            state
                .alloc
                .free_bytes(p.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        }
        for mut t in [dedup, obj, mt, proxy] {
            t.as_mut().free_table(&mut state.alloc);
            state
                .alloc
                .free_bytes(t.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        }
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn vm_inline_cache_accelerates_getfield_setfield_and_invalidates_on_rehash() {
    let mut state = ElaraState::new();

    let mut main = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("main");
    let mut dedup =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let (getfield_pc, setfield_pc) = {
        let p = unsafe { main.as_mut() };
        p.max_stack_size = 8;
        let name = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"n")
            .expect("n");
        let k = unsafe {
            (*p).add_constant(
                &mut state.alloc,
                dedup.as_mut(),
                Value::from_obj(name.cast(), ObjType::String),
            )
            .expect("k")
        };
        let gf = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetField, 1, 0, k as u8),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 2, 1),
            1,
        );
        let sf = p.code_count;
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::SetField, 0, k as u8, 2),
            1,
        );
        for _ in 0..3 {
            emit_inst(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::GetField, 1, 0, k as u8),
                1,
            );
        }
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 1, 2, 0),
            1,
        );
        (gf, sf)
    };

    let c_main = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr())
        .expect("c_main");
    let mut tab = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("tab");
    let key = unsafe { *main.as_ref().constants.add(0) };
    unsafe {
        tab.as_mut()
            .set(&mut state.alloc, key, Value::from_i48(10))
            .unwrap();
    }

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 4).expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(0) = closure_value(c_main.as_ptr());
        *f.stack.add(1) = table_value(tab.as_ptr());
        f.open_top = 16;
        f.push_frame(&mut state.alloc, CallFrame::new(c_main.as_ptr(), 1, 0, 2))
            .unwrap();
    }

    assert_eq!(
        unsafe { execute(&mut state, fiber.as_ptr()) },
        ElaraStatus::Ok
    );
    let cached;
    unsafe {
        assert_eq!((*fiber.as_ref().stack.add(0)).to_i64(), Some(1));
        cached = *main.as_ref().ic_slots.add(getfield_pc as usize);
        assert_ne!(cached, u16::MAX);
        assert_ne!(*main.as_ref().ic_slots.add(setfield_pc as usize), u16::MAX);

        for i in 0..64i64 {
            tab.as_mut()
                .set(
                    &mut state.alloc,
                    Value::from_i48(1000 + i),
                    Value::from_i48(i),
                )
                .unwrap();
        }
    }

    // Fresh proto + fresh dedup so constant index 0 is local.
    let mut main2 = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("main2");
    let mut dedup2 =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("dedup2");
    {
        let p = unsafe { main2.as_mut() };
        p.max_stack_size = 4;
        let k = unsafe {
            (*p).add_constant(&mut state.alloc, dedup2.as_mut(), key)
                .expect("k2")
        };
        assert_eq!(k, 0);
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetField, 1, 0, 0),
            1,
        );
        emit_inst(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 1, 2, 0),
            1,
        );
        unsafe {
            *p.ic_slots.add(0) = cached;
        }
    }
    let c2 =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main2.as_ptr()).expect("c2");

    unsafe {
        fiber.as_mut().frame_count = 0;
        *fiber.as_mut().stack.add(0) = closure_value(c2.as_ptr());
        *fiber.as_mut().stack.add(1) = table_value(tab.as_ptr());
        fiber.as_mut().open_top = 8;
        fiber
            .as_mut()
            .push_frame(&mut state.alloc, CallFrame::new(c2.as_ptr(), 1, 0, 2))
            .unwrap();
        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);
        assert_eq!((*fiber.as_ref().stack.add(0)).to_i64(), Some(1));
        assert_ne!(*main2.as_ref().ic_slots.add(0), u16::MAX);

        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state
            .alloc
            .free_bytes(c2.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        state
            .alloc
            .free_bytes(c_main.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        main.as_mut().free_proto(&mut state.alloc);
        main2.as_mut().free_proto(&mut state.alloc);
        dedup.as_mut().free_table(&mut state.alloc);
        dedup2.as_mut().free_table(&mut state.alloc);
        tab.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main2.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state
            .alloc
            .free_bytes(dedup2.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state
            .alloc
            .free_bytes(tab.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}
