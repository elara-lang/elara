#![allow(unsafe_op_in_unsafe_fn)]

use core::ptr::NonNull;

use elara::compiler::{Instruction, ObjProto, OpCode, UpvalDesc};
use elara::value::{ObjTable, ObjType, Value};
use elara::vm::{
    ELAC_HEADER_SIZE, ElaraState, ElaraStatus, ObjClosure, deserialize_elac, serialize_elac,
};

fn emit(p: &mut ObjProto, state: &mut ElaraState, inst: Instruction, line: u32) {
    p.emit_instruction(&mut state.alloc, inst, line)
        .expect("emit");
}

unsafe fn build_nested_proto(state: &mut ElaraState) -> *mut ObjProto {
    let mut dedup =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");

    // Child: return K[0] where K[0] = "hi"
    let mut child = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("child");
    {
        let p = child.as_mut();
        p.max_stack_size = 2;
        p.num_params = 0;
        let s = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"hi")
            .expect("str");
        let k = p
            .add_constant(
                &mut state.alloc,
                dedup.as_mut(),
                Value::from_obj(s.cast(), ObjType::String),
            )
            .unwrap();
        emit(p, state, Instruction::make_abx(OpCode::LoadK, 0, k), 1);
        emit(p, state, Instruction::make_abc(OpCode::Return, 0, 2, 0), 1);
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
    }

    // Root with mixed constants + nested child
    let name = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, b"chunk")
        .expect("name");
    let mut root =
        ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, name.as_ptr()).expect("root");
    {
        let p = root.as_mut();
        p.max_stack_size = 4;
        p.num_params = 1;
        p.is_vararg = 1;

        p.add_constant(&mut state.alloc, dedup.as_mut(), Value::NIL)
            .unwrap();
        p.add_constant(&mut state.alloc, dedup.as_mut(), Value::TRUE)
            .unwrap();
        p.add_constant(&mut state.alloc, dedup.as_mut(), Value::from_i48(42))
            .unwrap();
        p.add_constant(&mut state.alloc, dedup.as_mut(), Value::from_f64(3.5))
            .unwrap();

        // Boxed i64 outside i48 range
        let raw = state
            .alloc
            .alloc_bytes(core::mem::size_of::<elara::value::ObjBoxedI64>())
            .expect("box");
        let bp = raw.as_ptr().cast::<elara::value::ObjBoxedI64>();
        core::ptr::write(bp, elara::value::ObjBoxedI64::new(i64::MAX, 0));
        (*bp).header.next = state.all_objects;
        state.all_objects = bp.cast();
        p.add_constant(
            &mut state.alloc,
            dedup.as_mut(),
            Value::from_obj(NonNull::new(bp.cast()).unwrap(), ObjType::BoxedI64),
        )
        .unwrap();

        emit(p, state, Instruction::make_abx(OpCode::LoadK, 0, 2), 10);
        emit(p, state, Instruction::make_abx(OpCode::Closure, 1, 0), 11);
        emit(p, state, Instruction::make_abc(OpCode::Return, 0, 2, 0), 12);

        p.add_child_proto(&mut state.alloc, child.as_ptr()).unwrap();
    }

    // Unlink temporary dedup
    let d = dedup.as_ptr();
    let mut cursor = &mut state.all_objects;
    while !(*cursor).is_null() {
        if *cursor == d.cast() {
            *cursor = (*d.cast::<elara::value::GcHeader>()).next;
            break;
        }
        cursor = &mut (*(*cursor)).next;
    }
    dedup.as_mut().free_table(&mut state.alloc);
    state
        .alloc
        .free_bytes(d.cast::<u8>(), core::mem::size_of::<ObjTable>());

    root.as_ptr()
}

#[test]
fn bytecode_serializer_and_loader_round_trip_nested_closures_and_constants() {
    let mut state = ElaraState::new();
    unsafe {
        let root = build_nested_proto(&mut state);
        let buf = serialize_elac(root, 0).expect("serialize");
        assert!(buf.len() > ELAC_HEADER_SIZE);
        assert_eq!(&buf[0..6], b"\x1bELARA");

        let loaded = deserialize_elac(&mut state, &buf).expect("deserialize");
        assert_eq!((*loaded).num_params, 1);
        assert_eq!((*loaded).is_vararg, 1);
        assert_eq!((*loaded).max_stack_size, 4);
        assert_eq!((*loaded).const_count, 5);
        assert_eq!((*loaded).proto_count, 1);
        assert_eq!((*loaded).code_count, 3);
        assert!(!(*loaded).source_name.is_null());
        assert_eq!((*(*loaded).source_name).as_bytes(), b"chunk");

        assert!((*(*loaded).constants.add(0)).is_nil());
        assert!((*(*loaded).constants.add(1)).as_bool());
        assert_eq!((*(*loaded).constants.add(2)).to_i64(), Some(42));
        assert!((*(*loaded).constants.add(3)).is_float());
        assert!((*(*loaded).constants.add(4)).is_obj_type(ObjType::BoxedI64));

        let child = *(*loaded).protos.add(0);
        assert_eq!((*child).const_count, 1);
        assert_eq!((*child).upvalue_count, 1);
        assert_eq!((*(*child).upvalues).is_local, 1);
        assert!((*(*child).constants).is_obj_type(ObjType::String));
        assert_eq!(
            (*(*(*child).constants)
                .as_obj()
                .cast::<elara::value::ObjString>())
            .as_bytes(),
            b"hi"
        );

        // Also via C loader.
        let mut out = 0u64;
        let st =
            elara::ffi::c_api::elara_load_bytecode(&mut state, buf.as_ptr(), buf.len(), &mut out);
        assert_eq!(st, ElaraStatus::Ok);
        assert!(Value(out).is_obj_type(ObjType::Closure));
        let _ = (root, out, core::mem::size_of::<ObjClosure>());
    }
}

#[test]
fn loader_rejects_corrupted_magic_header_and_crc32c_mismatch_with_err_verify() {
    let mut state = ElaraState::new();
    unsafe {
        let root = build_nested_proto(&mut state);
        let mut buf = serialize_elac(root, 0).expect("serialize");

        // Bad magic
        let mut bad_magic = buf.clone();
        bad_magic[1] = b'X';
        let err = deserialize_elac(&mut state, &bad_magic).unwrap_err();
        assert_eq!(err, ElaraStatus::ErrVerify);
        assert_eq!(state.last_error, Some("elac: bad magic"));

        // CRC mismatch: flip a payload byte
        let mut bad_crc = buf.clone();
        let flip = ELAC_HEADER_SIZE + 8;
        bad_crc[flip] ^= 0xFF;
        let err = deserialize_elac(&mut state, &bad_crc).unwrap_err();
        assert_eq!(err, ElaraStatus::ErrVerify);
        assert_eq!(state.last_error, Some("elac: crc32c mismatch"));

        // Truncated header
        let err = deserialize_elac(&mut state, &buf[..10]).unwrap_err();
        assert_eq!(err, ElaraStatus::ErrVerify);

        let _ = (root, &mut buf);
    }
}

#[test]
fn static_verifier_rejects_out_of_bounds_register_constant_upvalue_and_jump_targets() {
    use elara::vm::verify_proto;

    let mut state = ElaraState::new();
    unsafe {
        // Register out of bounds: Move A=10 with max_stack_size=2
        let mut p = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("p");
        {
            let proto = p.as_mut();
            proto.max_stack_size = 2;
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::Move, 10, 0, 0),
                1,
            );
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                1,
            );
        }
        assert!(verify_proto(p.as_ref(), None).is_err());

        // Constant Bx out of bounds
        let mut p2 = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("p2");
        {
            let proto = p2.as_mut();
            proto.max_stack_size = 2;
            emit(
                proto,
                &mut state,
                Instruction::make_abx(OpCode::LoadK, 0, 5),
                1,
            );
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                1,
            );
        }
        assert_eq!(
            verify_proto(p2.as_ref(), None).unwrap_err(),
            "verify: LoadK/Import Bx out of bounds"
        );

        // Upvalue B out of bounds
        let mut p3 = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("p3");
        {
            let proto = p3.as_mut();
            proto.max_stack_size = 2;
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::GetUpval, 0, 1, 0),
                1,
            );
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                1,
            );
        }
        assert_eq!(
            verify_proto(p3.as_ref(), None).unwrap_err(),
            "verify: upvalue B out of bounds"
        );

        // Jump target out of bounds
        let mut p4 = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("p4");
        {
            let proto = p4.as_mut();
            proto.max_stack_size = 2;
            emit(proto, &mut state, Instruction::make_sj(OpCode::Jmp, 99), 1);
        }
        assert_eq!(
            verify_proto(p4.as_ref(), None).unwrap_err(),
            "verify: jump target out of bounds"
        );

        // Last instruction neither RETURN nor JMP
        let mut p5 = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("p5");
        {
            let proto = p5.as_mut();
            proto.max_stack_size = 2;
            emit(
                proto,
                &mut state,
                Instruction::make_abc(OpCode::Move, 0, 0, 0),
                1,
            );
        }
        assert_eq!(
            verify_proto(p5.as_ref(), None).unwrap_err(),
            "verify: last instruction must be RETURN or JMP"
        );

        // Child upvalue local index out of parent max_stack_size
        let mut parent = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("parent");
        let mut child = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("child");
        {
            let c = child.as_mut();
            c.max_stack_size = 2;
            c.add_upvalue(
                &mut state.alloc,
                UpvalDesc {
                    index: 7,
                    is_local: 1,
                    is_const: 0,
                    reserved: 0,
                },
            )
            .unwrap();
            emit(
                c,
                &mut state,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                1,
            );
        }
        {
            let p = parent.as_mut();
            p.max_stack_size = 2;
            p.add_child_proto(&mut state.alloc, child.as_ptr()).unwrap();
            emit(
                p,
                &mut state,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                1,
            );
        }
        assert_eq!(
            verify_proto(parent.as_ref(), None).unwrap_err(),
            "verify: upvalue local index out of bounds"
        );

        // Loader path rejects invalid bytecode after CRC-valid serialize of bad proto
        let mut bad = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("bad");
        {
            let proto = bad.as_mut();
            proto.max_stack_size = 2;
            emit(proto, &mut state, Instruction::make_sj(OpCode::Jmp, 50), 1);
        }
        let buf = serialize_elac(bad.as_ptr(), 0).expect("ser");
        let err = deserialize_elac(&mut state, &buf).unwrap_err();
        assert_eq!(err, ElaraStatus::ErrVerify);
    }
}
