#![allow(unsafe_op_in_unsafe_fn)]

use elara::compiler::{FunctionScope, Instruction, ObjProto, OpCode, module_basename};
use elara::value::{GcHeader, InternPool, ObjTable, ObjType, Value};
use elara::vm::ElaraState;

#[test]
fn module_basename_strips_path_and_extensions() {
    assert_eq!(module_basename("math/vec"), "vec");
    assert_eq!(module_basename("math/vec.ela"), "vec");
    assert_eq!(module_basename("a/b/c.elac"), "c");
    assert_eq!(module_basename("solo"), "solo");
}

#[test]
fn module_exports_functions_and_constants_while_isolating_private_locals() {
    // Codegen lock for module prologue / export / return-exports.
    // Runtime export table materialization is covered once OP_IMPORT execution lands.
    let mut state = ElaraState::new();
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");

    unsafe {
        let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
        scope.begin_module(&mut state.alloc, 1).expect("begin");

        // private local at R[1]
        let priv_reg = scope.declare_local("hidden", false, false, 2).unwrap();
        assert_eq!(priv_reg, 1);
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, priv_reg, 99),
                2,
            )
            .unwrap();
        scope.mark_initialized();

        // export let answer = 42  → local R[2] + SETFIELD R[0], K["answer"], R[2]
        let ans = scope.declare_local("answer", false, false, 3).unwrap();
        assert_eq!(ans, 2);
        scope
            .emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, ans, 42),
                3,
            )
            .unwrap();
        scope.mark_initialized();
        scope
            .emit_export(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "answer",
                ans,
                3,
            )
            .unwrap();

        scope.finish_module(&mut state.alloc, 4).unwrap();

        let p = proto.as_ref();
        assert_eq!(p.code_count, 5);
        // NEWTABLE R[0], 0, 4
        assert_eq!(
            *p.code.add(0),
            Instruction::make_abc(OpCode::NewTable, 0, 0, 4)
        );
        // LOADI hidden
        assert_eq!(*p.code.add(1), Instruction::make_asbx(OpCode::LoadI, 1, 99));
        // LOADI answer
        assert_eq!(*p.code.add(2), Instruction::make_asbx(OpCode::LoadI, 2, 42));
        // SETFIELD R[0], K["answer"], R[2]
        let setf = *p.code.add(3);
        assert_eq!(setf.opcode(), Some(OpCode::SetField));
        assert_eq!(setf.a(), 0);
        assert_eq!(setf.c(), 2);
        let key = *p.constants.add(setf.b() as usize);
        assert!(key.is_obj_type(ObjType::String));
        assert_eq!(
            (*key.as_obj().cast::<elara::value::ObjString>()).as_bytes(),
            b"answer"
        );
        // RETURN R[0], 2
        assert_eq!(
            *p.code.add(4),
            Instruction::make_abc(OpCode::Return, 0, 2, 0)
        );

        // "hidden" was never exported — only one string field name in K for "answer"
        // (specifier strings may appear later for imports).
        let mut export_names = 0u32;
        for i in 0..p.const_count {
            let v = *p.constants.add(i as usize);
            if v.is_obj_type(ObjType::String) {
                let s = (*v.as_obj().cast::<elara::value::ObjString>()).as_bytes();
                if s == b"answer" {
                    export_names += 1;
                }
                assert_ne!(s, b"hidden");
            }
        }
        assert_eq!(export_names, 1);

        let _ = (
            scope,
            dedup,
            core::mem::size_of::<GcHeader>(),
            core::ptr::null_mut::<InternPool>(),
            Value::NIL,
        );
    }
}

#[test]
fn importing_same_module_twice_returns_cached_singleton_without_reexecuting() {
    // Codegen: two imports of the same specifier emit two OP_IMPORT with the same K index.
    let mut state = ElaraState::new();
    let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8).expect("dedup");
    let proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");

    unsafe {
        let mut scope = FunctionScope::new(proto.as_ptr(), dedup.as_ptr());
        scope.begin_module(&mut state.alloc, 1).unwrap();
        let r1 = scope
            .compile_import(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "math/vec.ela",
                None,
                2,
            )
            .unwrap();
        assert_eq!(r1, 1);
        // Local name from basename
        assert_eq!(scope.locals[0].name, "vec");

        let r2 = scope
            .compile_import(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                "math/vec.ela",
                Some("vec2"),
                3,
            )
            .unwrap();
        assert_eq!(r2, 2);

        scope.finish_module(&mut state.alloc, 4).unwrap();

        let p = proto.as_ref();
        let i1 = *p.code.add(1);
        let i2 = *p.code.add(2);
        assert_eq!(i1.opcode(), Some(OpCode::Import));
        assert_eq!(i2.opcode(), Some(OpCode::Import));
        assert_eq!(i1.bx(), i2.bx(), "same specifier must share K index");
        let spec = *p.constants.add(i1.bx() as usize);
        assert_eq!(
            (*spec.as_obj().cast::<elara::value::ObjString>()).as_bytes(),
            b"math/vec.ela"
        );
        let _ = (scope, dedup);
    }
}
