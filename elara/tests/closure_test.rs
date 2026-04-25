use core::mem::size_of;

use elara::compiler::{Instruction, ObjProto, OpCode, UpvalDesc};
use elara::gc::GcAllocator;
use elara::value::{ObjTable, ObjType, Value};
use elara::vm::{
    CallFrame, ElaraState, ElaraStatus, ObjClosure, ObjFiber, close_upvalues_from, execute,
    fill_upvalues,
};

fn emit(p: &mut ObjProto, alloc: &mut GcAllocator, inst: Instruction, line: u32) {
    p.emit_instruction(alloc, inst, line).expect("emit");
}

fn closure_val(c: *mut ObjClosure) -> Value {
    Value::from_obj(
        core::ptr::NonNull::new(c.cast()).expect("non-null"),
        ObjType::Closure,
    )
}

#[test]
fn two_closures_capturing_same_local_share_mutations_before_and_after_return() {
    // maker: R[0]=local; CLOSURE getter; CLOSURE setter; CLOSE; return both.
    // setter writes via SETUPVAL; getter reads via GETUPVAL — same upvalue.
    let mut state = ElaraState::new();

    let mut getter = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("getter");
    unsafe {
        let p = getter.as_mut();
        p.max_stack_size = 2;
        p.add_upvalue(
            &mut state.alloc,
            UpvalDesc {
                index: 0,
                is_local: 1,
                is_const: 0,
                reserved: 0,
            },
        )
        .unwrap();
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetUpval, 0, 0, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            1,
        );
    }

    let mut setter = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("setter");
    unsafe {
        let p = setter.as_mut();
        p.num_params = 1;
        p.max_stack_size = 2;
        p.add_upvalue(
            &mut state.alloc,
            UpvalDesc {
                index: 0,
                is_local: 1,
                is_const: 0,
                reserved: 0,
            },
        )
        .unwrap();
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::SetUpval, 0, 0, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 1, 0),
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
    unsafe {
        let p = main.as_mut();
        p.max_stack_size = 8;
        let g = p
            .add_child_proto(&mut state.alloc, getter.as_ptr())
            .unwrap();
        let s = p
            .add_child_proto(&mut state.alloc, setter.as_ptr())
            .unwrap();
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 0, 10),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abx(OpCode::Closure, 1, g),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abx(OpCode::Closure, 2, s),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Close, 0, 0, 0),
            1,
        );
        // setter(20)
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 3, 2, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 4, 20),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 3, 2, 1),
            1,
        );
        // getter() -> R[5]
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 5, 1, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 5, 1, 2),
            1,
        );
        // setter(30); getter() -> R[6]
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 3, 2, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, 4, 30),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 3, 2, 1),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Move, 6, 1, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, 6, 1, 2),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 5, 3, 0),
            1,
        );
    }

    let mut c_main =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr()).expect("cm");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(0) = closure_val(c_main.as_ptr());
        f.open_top = 32;
        f.push_frame(&mut state.alloc, CallFrame::new(c_main.as_ptr(), 1, 0, 2))
            .unwrap();
        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);
        assert_eq!((*f.stack.add(0)).to_i64(), Some(20));
        assert_eq!((*f.stack.add(1)).to_i64(), Some(30));

        let cg = (*f.stack.add(1 + 1)).as_obj().cast::<ObjClosure>(); // R[1]
        let cs = (*f.stack.add(1 + 2)).as_obj().cast::<ObjClosure>(); // R[2]
        // After return, stack[0]=20, stack[1]=30 — closures were at R[1],R[2] before return
        // Return overwrote from R[5]; closures may still sit above. Re-read from open list empty.
        let _ = (cg, cs);
        // Free child closures still referenced from stack slots above returns if present.
        // Getter/setter live at stack[2], stack[3] before return clobber — after Return 5,3
        // only stack[0],stack[1] are results. Closures remain at stack[2], stack[3].
        let g_cl = (*f.stack.add(2)).as_obj().cast::<ObjClosure>();
        let s_cl = (*f.stack.add(3)).as_obj().cast::<ObjClosure>();
        assert_eq!(*(*g_cl).upvalues.add(0), *(*s_cl).upvalues.add(0));
        let uv = *(*g_cl).upvalues.add(0);
        assert_eq!((*uv).stack_idx, u32::MAX);
        assert_eq!((*(*uv).location).to_i64(), Some(30));

        (*uv).free_upvalue(&mut state.alloc);
        (*g_cl).free_closure(&mut state.alloc);
        (*s_cl).free_closure(&mut state.alloc);
        c_main.as_mut().free_closure(&mut state.alloc);
        getter.as_mut().free_proto(&mut state.alloc);
        setter.as_mut().free_proto(&mut state.alloc);
        main.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(getter.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(setter.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn for_loop_closures_capture_per_iteration_let_bindings_independently() {
    // Each loop iteration: LoadI R[1]=i; CLOSURE R[2+i]; CLOSE R[1] so the next
    // iteration's let binding does not share the previous upvalue.
    let mut state = ElaraState::new();

    let mut child = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("child");
    unsafe {
        let p = child.as_mut();
        p.num_params = 0;
        p.max_stack_size = 2;
        p.add_upvalue(
            &mut state.alloc,
            UpvalDesc {
                index: 1, // capture parent R[1]
                is_local: 1,
                is_const: 0,
                reserved: 0,
            },
        )
        .expect("uv");
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::GetUpval, 0, 0, 0),
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
    unsafe {
        let p = main.as_mut();
        p.max_stack_size = 16;
        let child_idx = p
            .add_child_proto(&mut state.alloc, child.as_ptr())
            .expect("child");
        for i in 0..3i32 {
            emit(
                p,
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, 1, i),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, (2 + i) as u8, child_idx),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Close, 1, 0, 0),
                1,
            );
        }
        // Call each closure: Move to R[8], Call, Move result to R[5+i]
        for i in 0..3u8 {
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Move, 8, 2 + i, 0),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Call, 8, 1, 2),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Move, 5 + i, 8, 0),
                1,
            );
        }
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 5, 4, 0),
            1,
        );
    }

    let mut c_main =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr()).expect("cm");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(0) = closure_val(c_main.as_ptr());
        f.open_top = 32;
        f.push_frame(&mut state.alloc, CallFrame::new(c_main.as_ptr(), 1, 0, 3))
            .expect("frame");

        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);
        assert_eq!((*f.stack.add(0)).to_i64(), Some(0));
        assert_eq!((*f.stack.add(1)).to_i64(), Some(1));
        assert_eq!((*f.stack.add(2)).to_i64(), Some(2));

        // Closures at R[2..4] (stack 3..5) must hold distinct closed upvalues.
        let c0 = (*f.stack.add(3)).as_obj().cast::<ObjClosure>();
        let c1 = (*f.stack.add(4)).as_obj().cast::<ObjClosure>();
        let c2 = (*f.stack.add(5)).as_obj().cast::<ObjClosure>();
        let u0 = *(*c0).upvalues.add(0);
        let u1 = *(*c1).upvalues.add(0);
        let u2 = *(*c2).upvalues.add(0);
        assert_ne!(u0, u1);
        assert_ne!(u1, u2);
        *(*u1).location = Value::from_i48(99);
        assert_eq!((*(*u0).location).to_i64(), Some(0));
        assert_eq!((*(*u2).location).to_i64(), Some(2));

        for c in [c0, c1, c2] {
            let uv = *(*c).upvalues.add(0);
            (*uv).free_upvalue(&mut state.alloc);
            (*c).free_closure(&mut state.alloc);
        }
        c_main.as_mut().free_closure(&mut state.alloc);
        child.as_mut().free_proto(&mut state.alloc);
        main.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(child.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn nested_3_level_closures_capture_and_mutate_upvalues_across_stack_realloc() {
    let mut state = ElaraState::new();

    let mut outer = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("outer");
    unsafe {
        outer.as_mut().max_stack_size = 4;
    }

    let mut mid = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("mid");
    unsafe {
        mid.as_mut().max_stack_size = 4;
        mid.as_mut()
            .add_upvalue(
                &mut state.alloc,
                UpvalDesc {
                    index: 0,
                    is_local: 1,
                    is_const: 0,
                    reserved: 0,
                },
            )
            .expect("uv");
    }

    let mut inner = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("inner");
    unsafe {
        inner.as_mut().max_stack_size = 4;
        inner
            .as_mut()
            .add_upvalue(
                &mut state.alloc,
                UpvalDesc {
                    index: 0,
                    is_local: 0,
                    is_const: 0,
                    reserved: 0,
                },
            )
            .expect("uv");
    }

    let mut c_outer =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, outer.as_ptr()).expect("co");
    let mut c_mid =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, mid.as_ptr()).expect("cm");
    let mut c_inner =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, inner.as_ptr()).expect("ci");

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 8, 4).expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(1) = Value::from_i48(7);
        f.open_top = 4;

        fill_upvalues(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            f,
            c_mid.as_ptr(),
            c_outer.as_ptr(),
            1,
        )
        .expect("mid fill");
        fill_upvalues(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            f,
            c_inner.as_ptr(),
            c_mid.as_ptr(),
            1,
        )
        .expect("inner fill");

        let uv_mid = *c_mid.as_ref().upvalues.add(0);
        let uv_inner = *c_inner.as_ref().upvalues.add(0);
        assert_eq!(uv_mid, uv_inner);

        let old_loc = (*uv_mid).location;
        f.open_top = f.stack_capacity;
        let grow_to = f.stack_capacity + 64;
        f.ensure_stack(&mut state.alloc, grow_to).expect("grow");
        assert_ne!((*uv_mid).location, old_loc);
        assert_eq!((*uv_mid).location, f.stack.add(1));
        assert_eq!((*(*uv_inner).location).to_i64(), Some(7));

        *(*uv_inner).location = Value::from_i48(42);
        assert_eq!((*f.stack.add(1)).to_i64(), Some(42));

        close_upvalues_from(&mut state, f, 1).expect("close");
        *(*uv_mid).location = Value::from_i48(100);
        assert_eq!((*(*uv_inner).location).to_i64(), Some(100));

        (*uv_mid).free_upvalue(&mut state.alloc);
        c_outer.as_mut().free_closure(&mut state.alloc);
        c_mid.as_mut().free_closure(&mut state.alloc);
        c_inner.as_mut().free_closure(&mut state.alloc);
        outer.as_mut().free_proto(&mut state.alloc);
        mid.as_mut().free_proto(&mut state.alloc);
        inner.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(outer.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(mid.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(inner.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn op_close_executes_defer_blocks_and_close_metamethods_in_lifo_order() {
    let mut state = ElaraState::new();

    let mut defer_proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("defer_proto");
    let mut defer_fn = ObjClosure::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        defer_proto.as_ptr(),
    )
    .expect("defer_fn");

    let mut res = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("res");
    let mut mt = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("mt");
    let mut close_mm =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 0).expect("mm");
    unsafe {
        mt.as_mut()
            .set(
                &mut state.alloc,
                state.mm_close,
                Value::from_obj(close_mm.cast(), ObjType::Table),
            )
            .unwrap();
        res.as_mut().metatable = mt.as_ptr();
    }

    let mut main = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("main");
    unsafe {
        let p = main.as_mut();
        p.max_stack_size = 8;
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Close, 1, 0, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 1, 0),
            1,
        );
    }
    let mut c_main =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, main.as_ptr()).expect("cm");

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 4).expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        // R[1]=defer closure, R[2]=resource — absolute slots 2 and 3 with base_idx=1.
        *f.stack.add(0) = closure_val(c_main.as_ptr());
        *f.stack.add(2) = closure_val(defer_fn.as_ptr());
        *f.stack.add(3) = Value::from_obj(res.cast(), ObjType::Table);
        f.open_top = 8;
        // close_slots store absolute stack indices.
        f.push_close_slot(&mut state.alloc, 2).unwrap();
        f.push_close_slot(&mut state.alloc, 3).unwrap();
        f.push_frame(&mut state.alloc, CallFrame::new(c_main.as_ptr(), 1, 0, 1))
            .unwrap();

        state.clear_close_hook_log();
        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);

        assert_eq!(state.close_hook_log_len, 2);
        assert!(
            state.close_hook_log[0].is_obj_type(ObjType::Table),
            "resource __close first"
        );
        assert!(
            state.close_hook_log[1].is_obj_type(ObjType::Closure),
            "defer closure second"
        );
        assert_eq!(f.close_slot_count, 0);

        defer_fn.as_mut().free_closure(&mut state.alloc);
        c_main.as_mut().free_closure(&mut state.alloc);
        defer_proto.as_mut().free_proto(&mut state.alloc);
        main.as_mut().free_proto(&mut state.alloc);
        res.as_mut().free_table(&mut state.alloc);
        mt.as_mut().free_table(&mut state.alloc);
        close_mm.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(defer_proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(main.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(res.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state
            .alloc
            .free_bytes(mt.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        state
            .alloc
            .free_bytes(close_mm.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}
