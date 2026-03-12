use core::mem::size_of;
use core::ptr::NonNull;

use elara::compiler::{
    ExprKind, FunctionScope, Instruction, ObjProto, OpCode, UpvalDesc, resolve_assignable,
    resolve_variable,
};
use elara::gc::GcAllocator;
use elara::value::{GcHeader, InternPool, ObjBoxedI64, ObjTable, ObjType, Value};

#[test]
fn constant_pool_deduplicates_identical_values_and_encodes_rle_lines() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();
    let mut pool = InternPool::new(0x1357_9BDF);

    let chunk_name = pool
        .intern_always(&mut alloc, 0, b"test_chunk.ela")
        .expect("chunk name");

    let mut proto_ptr = ObjProto::new(&mut alloc, &mut all_objects, 0, chunk_name.as_ptr())
        .expect("allocate ObjProto");
    let mut dedup_ptr =
        ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 8).expect("allocate dedup table");

    let proto = unsafe { proto_ptr.as_mut() };
    let dedup = unsafe { dedup_ptr.as_mut() };

    // 1. Constant pool deduplication across i48, f64, strings, and ObjBoxedI64
    let idx_int_1 = proto
        .add_constant(&mut alloc, dedup, Value::from_i48(42))
        .expect("add int 42");
    let idx_int_2 = proto
        .add_constant(&mut alloc, dedup, Value::from_i48(42))
        .expect("add duplicate int 42");
    assert_eq!(idx_int_1, 0);
    assert_eq!(idx_int_2, 0);

    // Note: float `42.0` must NOT collide with integer `42` in K[i] so type identity is preserved!
    let idx_flt_1 = proto
        .add_constant(&mut alloc, dedup, Value::from_f64(42.0))
        .expect("add float 42.0");
    let idx_flt_2 = proto
        .add_constant(&mut alloc, dedup, Value::from_f64(42.0))
        .expect("add duplicate float 42.0");
    assert_eq!(idx_flt_1, 1);
    assert_eq!(idx_flt_2, 1);

    let s1 = pool
        .intern_always(&mut alloc, 0, b"greeting")
        .expect("intern greeting");
    let v_str = Value::from_obj(s1.cast::<GcHeader>(), ObjType::String);
    let idx_str_1 = proto
        .add_constant(&mut alloc, dedup, v_str)
        .expect("add str");
    let idx_str_2 = proto
        .add_constant(&mut alloc, dedup, v_str)
        .expect("add duplicate str");
    assert_eq!(idx_str_1, 2);
    assert_eq!(idx_str_2, 2);

    // Two distinct ObjBoxedI64 instances with the same i64 value deduplicate to the same constant slot!
    let mut b1 = ObjBoxedI64::new(i64::MAX - 7, 0);
    let mut b2 = ObjBoxedI64::new(i64::MAX - 7, 0);
    let v_b1 = Value::from_obj(
        NonNull::new(&raw mut b1.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let v_b2 = Value::from_obj(
        NonNull::new(&raw mut b2.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let idx_b1 = proto
        .add_constant(&mut alloc, dedup, v_b1)
        .expect("add boxed 1");
    let idx_b2 = proto
        .add_constant(&mut alloc, dedup, v_b2)
        .expect("add boxed 2");
    assert_eq!(idx_b1, 3);
    assert_eq!(idx_b2, 3);
    assert_eq!(proto.const_count, 4);

    // 2. Emit instructions across source lines and verify RLE compression + `ic_slots` initialization
    let pc0 = proto
        .emit_instruction(&mut alloc, Instruction::make_asbx(OpCode::LoadI, 0, 10), 5)
        .expect("emit 0");
    let pc1 = proto
        .emit_instruction(&mut alloc, Instruction::make_asbx(OpCode::LoadI, 1, 20), 5)
        .expect("emit 1");
    let pc2 = proto
        .emit_instruction(&mut alloc, Instruction::make_abc(OpCode::Add, 2, 0, 1), 5)
        .expect("emit 2");
    let pc3 = proto
        .emit_instruction(
            &mut alloc,
            Instruction::make_abc(OpCode::Return, 2, 2, 0),
            6,
        )
        .expect("emit 3");

    assert_eq!((pc0, pc1, pc2, pc3), (0, 1, 2, 3));
    assert_eq!(proto.code_count, 4);
    assert_eq!(proto.line_run_count, 2); // 3 instructions on line 5, 1 instruction on line 6
    assert_eq!(proto.get_line(0), 5);
    assert_eq!(proto.get_line(1), 5);
    assert_eq!(proto.get_line(2), 5);
    assert_eq!(proto.get_line(3), 6);
    assert_eq!(proto.get_line(99), 0);

    // Verify parallel `ic_slots` are initialized to `0xFFFF` (`u16::MAX`)
    for pc in 0..proto.code_count {
        let ic = unsafe { *proto.ic_slots.add(pc as usize) };
        assert_eq!(ic, u16::MAX);
    }

    // 3. Upvalue descriptor deduplication
    let u0 = proto
        .add_upvalue(
            &mut alloc,
            UpvalDesc {
                index: 2,
                is_local: 1,
                is_const: 0,
                reserved: 0,
            },
        )
        .expect("upval 0");
    let u0_dup = proto
        .add_upvalue(
            &mut alloc,
            UpvalDesc {
                index: 2,
                is_local: 1,
                is_const: 0,
                reserved: 0,
            },
        )
        .expect("upval 0 dup");
    assert_eq!(u0, 0);
    assert_eq!(u0_dup, 0);
    assert_eq!(proto.upvalue_count, 1);

    // Clean up all heap objects and verify 0 leaked bytes
    proto.free_proto(&mut alloc);
    dedup.free_table(&mut alloc);
    unsafe {
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    pool.free_all(&mut alloc);
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_resolves_nested_block_locals_shadowing_and_rejects_self_init() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let mut dedup_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("dedup");

    let mut scopes = vec![FunctionScope::new(proto_ptr.as_ptr(), dedup_ptr.as_ptr())];
    scopes[0].begin_scope();

    // Declare `let x = ...` (depth is -1 before `mark_initialized`)
    let r_x = scopes[0]
        .declare_local("x", false, false, 1)
        .expect("declare x");
    assert_eq!(r_x, 0);

    // Reading `x` inside its own initializer MUST fail with exact error message
    let err = resolve_variable(&mut alloc, &mut scopes, "x", 1).expect_err("self init");
    assert_eq!(
        err.message,
        "Cannot read local variable in its own initializer"
    );

    scopes[0].mark_initialized();
    assert_eq!(
        resolve_variable(&mut alloc, &mut scopes, "x", 1),
        Ok(ExprKind::Local {
            reg: 0,
            is_const: false
        })
    );

    // Shadow `x` in an inner block scope -> allocates R[1], resolves to R[1], and restores R[0] on exit
    scopes[0].begin_scope();
    let r_inner_x = scopes[0]
        .declare_local("x", true, false, 2)
        .expect("shadow x");
    assert_eq!(r_inner_x, 1);
    scopes[0].mark_initialized();

    assert_eq!(
        resolve_variable(&mut alloc, &mut scopes, "x", 2),
        Ok(ExprKind::Local {
            reg: 1,
            is_const: true
        })
    );

    scopes[0].end_scope(&mut alloc, 3).expect("end inner scope");
    assert_eq!(scopes[0].free_reg, 1);
    assert_eq!(
        resolve_variable(&mut alloc, &mut scopes, "x", 3),
        Ok(ExprKind::Local {
            reg: 0,
            is_const: false
        })
    );

    unsafe {
        proto_ptr.as_mut().free_proto(&mut alloc);
        dedup_ptr.as_mut().free_table(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_rejects_undeclared_identifiers_and_assignments_to_const_bindings() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let mut dedup_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("dedup");

    let mut scopes = vec![FunctionScope::new(proto_ptr.as_ptr(), dedup_ptr.as_ptr())];
    scopes[0].begin_scope();

    // Undeclared identifier MUST fail with exact message
    let undeclared =
        resolve_variable(&mut alloc, &mut scopes, "missing_var", 10).expect_err("undeclared");
    assert_eq!(undeclared.message, "Undeclared identifier 'missing_var'");

    // Const local variable MUST reject assignment with exact message
    scopes[0]
        .declare_local("MAX_LIMIT", true, false, 11)
        .expect("declare const");
    scopes[0].mark_initialized();

    let const_err =
        resolve_assignable(&mut alloc, &mut scopes, "MAX_LIMIT", 12).expect_err("assign const");
    assert_eq!(
        const_err.message,
        "Cannot assign to const variable 'MAX_LIMIT'"
    );

    unsafe {
        proto_ptr.as_mut().free_proto(&mut alloc);
        dedup_ptr.as_mut().free_table(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_inlines_primitive_consts_without_allocating_registers_or_upvalues() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut outer_proto =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("outer");
    let mut outer_dedup =
        ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("outer dedup");
    let mut inner_proto =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("inner");
    let mut inner_dedup =
        ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("inner dedup");

    let mut scopes = vec![
        FunctionScope::new(outer_proto.as_ptr(), outer_dedup.as_ptr()),
        FunctionScope::new(inner_proto.as_ptr(), inner_dedup.as_ptr()),
    ];

    // Declare `const ANSWER = 42` in outer scope as inline const
    scopes[0].begin_scope();
    scopes[0]
        .declare_inline_const("ANSWER", Value::from_i48(42), 1)
        .expect("inline const");

    // Verify 0 registers allocated in outer scope
    assert_eq!(scopes[0].free_reg, 0);

    // Reference `ANSWER` inside nested child closure (`scopes[1]`):
    // MUST resolve directly to `ExprKind::IntImm(42)` with 0 upvalues created on `inner_proto`!
    let resolved = resolve_variable(&mut alloc, &mut scopes, "ANSWER", 2).expect("resolve");
    assert_eq!(resolved, ExprKind::IntImm(42));
    assert_eq!(unsafe { (*inner_proto.as_ptr()).upvalue_count }, 0);

    // Mutating `ANSWER` inside nested closure MUST still fail with const error
    let assign_err =
        resolve_assignable(&mut alloc, &mut scopes, "ANSWER", 3).expect_err("assign inline const");
    assert_eq!(
        assign_err.message,
        "Cannot assign to const variable 'ANSWER'"
    );

    unsafe {
        outer_proto.as_mut().free_proto(&mut alloc);
        outer_dedup.as_mut().free_table(&mut alloc);
        inner_proto.as_mut().free_proto(&mut alloc);
        inner_dedup.as_mut().free_table(&mut alloc);
        alloc.free_bytes(outer_proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(outer_dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        alloc.free_bytes(inner_proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(inner_dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_resolves_multi_level_lexical_upvalues_and_emits_op_close() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut p0 = ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("p0");
    let mut d0 = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("d0");
    let mut p1 = ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("p1");
    let mut d1 = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("d1");
    let mut p2 = ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("p2");
    let mut d2 = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("d2");

    let mut scopes = vec![
        FunctionScope::new(p0.as_ptr(), d0.as_ptr()),
        FunctionScope::new(p1.as_ptr(), d1.as_ptr()),
        FunctionScope::new(p2.as_ptr(), d2.as_ptr()),
    ];

    // Declare `let counter = 0` at `R[0]` in level 0 block scope
    scopes[0].begin_scope();
    let reg = scopes[0]
        .declare_local("counter", false, false, 1)
        .expect("declare counter");
    assert_eq!(reg, 0);
    scopes[0].mark_initialized();

    // Resolve `counter` in level 2 (`p2` inside `p1` inside `p0`)
    let kind = resolve_variable(&mut alloc, &mut scopes, "counter", 5).expect("resolve upval");
    assert_eq!(
        kind,
        ExprKind::Upvalue {
            idx: 0,
            is_const: false
        }
    );

    // Verify `p1` captured `p0`'s local (`is_local == 1, index == 0`)
    // and `p2` captured `p1`'s upvalue (`is_local == 0, index == 0`)
    unsafe {
        assert_eq!((*p1.as_ptr()).upvalue_count, 1);
        let u1 = *(*p1.as_ptr()).upvalues;
        assert_eq!((u1.index, u1.is_local, u1.is_const), (0, 1, 0));

        assert_eq!((*p2.as_ptr()).upvalue_count, 1);
        let u2 = *(*p2.as_ptr()).upvalues;
        assert_eq!((u2.index, u2.is_local, u2.is_const), (0, 0, 0));
    }

    // Exiting level 0's block MUST emit `OP_CLOSE R[0]` onto `p0`
    let closed_reg = scopes[0].end_scope(&mut alloc, 10).expect("end scope");
    assert_eq!(closed_reg, Some(0));
    unsafe {
        assert_eq!((*p0.as_ptr()).code_count, 1);
        let inst = *(*p0.as_ptr()).code;
        assert_eq!(inst.opcode(), Some(OpCode::Close));
        assert_eq!(inst.a(), 0);
    }

    unsafe {
        p0.as_mut().free_proto(&mut alloc);
        d0.as_mut().free_table(&mut alloc);
        p1.as_mut().free_proto(&mut alloc);
        d1.as_mut().free_table(&mut alloc);
        p2.as_mut().free_proto(&mut alloc);
        d2.as_mut().free_table(&mut alloc);
        alloc.free_bytes(p0.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(d0.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        alloc.free_bytes(p1.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(d1.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        alloc.free_bytes(p2.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(d2.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}
