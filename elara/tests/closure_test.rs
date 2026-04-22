use core::mem::size_of;

use elara::compiler::{ObjProto, UpvalDesc};
use elara::value::{ObjTable, ObjType, Value};
use elara::vm::{
    ElaraState, ObjClosure, ObjFiber, ObjUpvalue, capture_upvalue, close_upvalues_from,
    fill_upvalues,
};

#[test]
fn two_closures_capturing_same_local_share_mutations_before_and_after_return() {
    let mut state = ElaraState::new();
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 2).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(2) = Value::from_i48(10);
        f.open_top = 8;

        let a = capture_upvalue(&mut state.alloc, &mut state.all_objects, 0, f, 2).expect("a");
        let b = capture_upvalue(&mut state.alloc, &mut state.all_objects, 0, f, 2).expect("b");
        assert_eq!(a, b, "same stack slot must share one ObjUpvalue");

        *(*a).location = Value::from_i48(20);
        assert_eq!((*f.stack.add(2)).to_i64(), Some(20));
        assert_eq!((*(*b).location).to_i64(), Some(20));

        close_upvalues_from(&mut state, f, 2).expect("close");
        assert!(f.open_upvalues.is_null());
        assert_eq!((*a).stack_idx, u32::MAX);
        assert_eq!((*(*a).location).to_i64(), Some(20));

        *(*a).location = Value::from_i48(30);
        assert_eq!((*(*b).location).to_i64(), Some(30));
        assert_eq!((*f.stack.add(2)).to_i64(), Some(20));

        (*a).free_upvalue(&mut state.alloc);
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
    let mut state = ElaraState::new();
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 2).expect("fiber");

    let mut captured: [*mut ObjUpvalue; 3] = [core::ptr::null_mut(); 3];
    unsafe {
        let f = fiber.as_mut();
        f.open_top = 16;
        for i in 0..3u32 {
            let slot = 4 + i;
            *f.stack.add(slot as usize) = Value::from_i48(i64::from(i));
            captured[i as usize] =
                capture_upvalue(&mut state.alloc, &mut state.all_objects, 0, f, slot).expect("cap");
        }
        assert_ne!(captured[0], captured[1]);
        assert_ne!(captured[1], captured[2]);

        close_upvalues_from(&mut state, f, 4).expect("close");
        assert_eq!((*(*captured[0]).location).to_i64(), Some(0));
        assert_eq!((*(*captured[1]).location).to_i64(), Some(1));
        assert_eq!((*(*captured[2]).location).to_i64(), Some(2));

        *(*captured[1]).location = Value::from_i48(99);
        assert_eq!((*(*captured[0]).location).to_i64(), Some(0));
        assert_eq!((*(*captured[2]).location).to_i64(), Some(2));

        for uv in captured {
            (*uv).free_upvalue(&mut state.alloc);
        }
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
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 2).expect("fiber");

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

    unsafe {
        let f = fiber.as_mut();
        // Declaration order: defer closure at 5, then resource at 6 → LIFO runs 6 then 5.
        *f.stack.add(5) = Value::from_obj(
            core::ptr::NonNull::new(defer_fn.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        *f.stack.add(6) = Value::from_obj(res.cast(), ObjType::Table);
        f.open_top = 8;
        f.push_close_slot(&mut state.alloc, 5).unwrap();
        f.push_close_slot(&mut state.alloc, 6).unwrap();

        state.clear_close_hook_log();
        close_upvalues_from(&mut state, f, 5).expect("close");

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
        defer_proto.as_mut().free_proto(&mut state.alloc);
        res.as_mut().free_table(&mut state.alloc);
        mt.as_mut().free_table(&mut state.alloc);
        close_mm.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(defer_proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
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
