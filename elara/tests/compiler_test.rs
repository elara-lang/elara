use core::mem::size_of;
use core::ptr::NonNull;

use elara::compiler::{
    BinOp, ExprDesc, ExprKind, FunctionScope, Instruction, NO_JUMP, ObjProto, OpCode, UnOp,
    UpvalDesc, encode_rk, newtable_hash_log2, resolve_assignable, resolve_variable,
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

#[test]
fn compiler_folds_constant_expressions_and_specializes_for_range_loops_without_op_range() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let mut dedup_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 8).expect("dedup");

    let mut scope = FunctionScope::new(proto_ptr.as_ptr(), dedup_ptr.as_ptr());
    scope.begin_scope();

    // 1. Constant folding: `2 + 3 * 4` and `not false` emit zero bytecode instructions
    let mul = scope
        .compile_binop(
            &mut alloc,
            BinOp::Mul,
            ExprDesc::new(ExprKind::IntImm(3)),
            ExprDesc::new(ExprKind::IntImm(4)),
            1,
        )
        .expect("fold mul");
    assert_eq!(mul.kind, ExprKind::IntImm(12));

    let add = scope
        .compile_binop(
            &mut alloc,
            BinOp::Add,
            ExprDesc::new(ExprKind::IntImm(2)),
            mul,
            1,
        )
        .expect("fold add");
    assert_eq!(add.kind, ExprKind::IntImm(14));

    let not_false = scope
        .compile_unop(&mut alloc, UnOp::Not, ExprDesc::new(ExprKind::False), 1)
        .expect("fold not");
    assert_eq!(not_false.kind, ExprKind::True);

    // Division by zero MUST NOT fold (left to runtime)
    let code_before_div0 = unsafe { (*proto_ptr.as_ptr()).code_count };
    let div0 = scope
        .compile_binop(
            &mut alloc,
            BinOp::Div,
            ExprDesc::new(ExprKind::IntImm(1)),
            ExprDesc::new(ExprKind::IntImm(0)),
            1,
        )
        .expect("div0");
    assert!(matches!(div0.kind, ExprKind::NonReloc { .. }));
    assert!(unsafe { (*proto_ptr.as_ptr()).code_count } > code_before_div0);

    // Pure folds above emitted no instructions before div0
    assert_eq!(code_before_div0, 0);

    // 2. Intrusive jump lists: concat pending JMPs and patch all to here
    let mut list = NO_JUMP;
    let j0 = scope.emit_jmp_link(&mut alloc, list, 2).expect("jmp0");
    list = j0;
    let j1 = scope.emit_jmp_link(&mut alloc, list, 2).expect("jmp1");
    list = j1;
    let j2 = scope.emit_jmp_link(&mut alloc, NO_JUMP, 2).expect("jmp2");
    scope.concat_jump_list(&mut list, j2);
    assert_eq!(list, j1);
    assert_eq!(scope.jump_list_next(j1), j0);
    assert_eq!(scope.jump_list_next(j0), j2);
    assert_eq!(scope.jump_list_next(j2), NO_JUMP);

    let _ = scope
        .emit(&mut alloc, Instruction::make_asbx(OpCode::LoadI, 0, 0), 2)
        .expect("target");
    let here = scope.pc();
    scope.patch_jump_list_to_here(list);
    unsafe {
        let p = proto_ptr.as_ptr();
        for jmp in [j0, j1, j2] {
            let inst = *(*p).code.add(jmp as usize);
            assert_eq!(inst.opcode(), Some(OpCode::Jmp));
            let dest = (jmp as i32 + 1 + inst.sj()) as u32;
            assert_eq!(dest, here);
        }
    }
    assert!(scope.last_target_pc > 0);

    // 3. `RK(C)` / `sC` / `OP_EQI` selection against a runtime local
    // Reset free_reg so the local lands in R[0] after temporary arith temps.
    scope.free_reg = 0;
    let reg_x = scope
        .declare_local("x", false, false, 3)
        .expect("declare x");
    scope.mark_initialized();
    assert_eq!(reg_x, 0);
    let x = ExprDesc::new(ExprKind::Local {
        reg: reg_x,
        is_const: false,
    });

    let before_addi = unsafe { (*proto_ptr.as_ptr()).code_count };
    let addi = scope
        .compile_binop(
            &mut alloc,
            BinOp::Add,
            x,
            ExprDesc::new(ExprKind::IntImm(5)),
            3,
        )
        .expect("addi");
    assert!(matches!(addi.kind, ExprKind::NonReloc { .. }));
    unsafe {
        let p = proto_ptr.as_ptr();
        let inst = *(*p).code.add(before_addi as usize);
        assert_eq!(inst.opcode(), Some(OpCode::AddI));
        assert_eq!(inst.b(), 0);
        assert_eq!(inst.sc(), 5);
    }

    // Sub with pool constant uses RK(C) when K index <= 127
    let k_idx = unsafe {
        (*proto_ptr.as_ptr())
            .add_constant(&mut alloc, dedup_ptr.as_mut(), Value::from_i48(100))
            .expect("K")
    };
    assert!(encode_rk(k_idx).is_some());
    // Drop temps so next emit reuses above locals; keep R[0] as `x`.
    scope.free_reg = 1;
    let before_sub = unsafe { (*proto_ptr.as_ptr()).code_count };
    let sub = scope
        .compile_binop(
            &mut alloc,
            BinOp::Sub,
            ExprDesc::new(ExprKind::Local {
                reg: reg_x,
                is_const: false,
            }),
            ExprDesc::new(ExprKind::Const(k_idx)),
            3,
        )
        .expect("sub rk");
    assert!(matches!(sub.kind, ExprKind::NonReloc { .. }));
    unsafe {
        let inst = *(*proto_ptr.as_ptr()).code.add(before_sub as usize);
        assert_eq!(inst.opcode(), Some(OpCode::Sub));
        assert_eq!(inst.c(), encode_rk(k_idx).unwrap());
    }

    scope.free_reg = 1;
    // Comparison against small int immediate emits OP_EQI + OP_JMP
    let before_eqi = unsafe { (*proto_ptr.as_ptr()).code_count };
    let cmp = scope
        .compile_binop(
            &mut alloc,
            BinOp::Eq,
            ExprDesc::new(ExprKind::Local {
                reg: reg_x,
                is_const: false,
            }),
            ExprDesc::new(ExprKind::IntImm(7)),
            4,
        )
        .expect("eqi");
    assert!(matches!(cmp.kind, ExprKind::Jump { .. }));
    unsafe {
        let p = proto_ptr.as_ptr();
        let eqi = *(*p).code.add(before_eqi as usize);
        assert_eq!(eqi.opcode(), Some(OpCode::EqI));
        assert_eq!(eqi.a(), 1);
        assert_eq!(eqi.sc(), 7);
        let jmp = *(*p).code.add((before_eqi + 1) as usize);
        assert_eq!(jmp.opcode(), Some(OpCode::Jmp));
    }

    // 4. Peephole condition inversion: `not (x == 7)` flips polarity, emits zero extra ops
    let code_before_not = unsafe { (*proto_ptr.as_ptr()).code_count };
    let inverted = scope
        .compile_unop(&mut alloc, UnOp::Not, cmp, 4)
        .expect("invert");
    assert_eq!(unsafe { (*proto_ptr.as_ptr()).code_count }, code_before_not);
    assert!(matches!(inverted.kind, ExprKind::Jump { .. }));
    unsafe {
        let eqi = *(*proto_ptr.as_ptr()).code.add(before_eqi as usize);
        assert_eq!(eqi.a(), 0); // polarity flipped 1 -> 0
    }

    // 5. Redundant `OP_MOVE R[A], R[A]` is suppressed
    let code_before_move = unsafe { (*proto_ptr.as_ptr()).code_count };
    scope.emit_move(&mut alloc, 0, 0, 5).expect("move nop");
    assert_eq!(
        unsafe { (*proto_ptr.as_ptr()).code_count },
        code_before_move
    );

    // 6. Range `for i in 0..10` specializes to FORPREP/FORLOOP with zero OP_RANGE
    scope.free_reg = 1;
    let info = scope
        .compile_range_for(
            &mut alloc,
            ExprDesc::new(ExprKind::IntImm(0)),
            ExprDesc::new(ExprKind::IntImm(10)),
            false,
            6,
            |_scope, _alloc, _loop_var| Ok(()),
        )
        .expect("range for");
    unsafe {
        let p = proto_ptr.as_ptr();
        let prep = *(*p).code.add(info.forprep_pc as usize);
        let loop_ = *(*p).code.add(info.forloop_pc as usize);
        assert_eq!(prep.opcode(), Some(OpCode::ForPrep));
        assert_eq!(loop_.opcode(), Some(OpCode::ForLoop));
        assert_eq!(prep.a(), info.loop_base);
        assert_eq!(loop_.a(), info.loop_base);
        for pc in 0..(*p).code_count {
            assert_ne!(
                (*(*p).code.add(pc as usize)).opcode(),
                Some(OpCode::Range),
                "OP_RANGE must not be emitted for syntactic range for-loops"
            );
        }
    }

    unsafe {
        proto_ptr.as_mut().free_proto(&mut alloc);
        dedup_ptr.as_mut().free_table(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_backpatches_newtable_capacities_and_evaluates_compound_assign_lhs_once() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let mut dedup_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 8).expect("dedup");

    let mut scope = FunctionScope::new(proto_ptr.as_ptr(), dedup_ptr.as_ptr());
    scope.begin_scope();

    // Allocate R[0] for the table and R[1..=3] as array element temps
    let dst = scope.alloc_regs(4, 1).expect("regs");
    assert_eq!(dst, 0);

    let mut builder = scope.begin_table(&mut alloc, dst, 1).expect("newtable");
    let newtable_pc = builder.newtable_pc;

    // Three array elements via SETLIST batch, two hash fields
    scope
        .table_setlist(&mut alloc, &mut builder, 3, 0, 1)
        .expect("setlist");
    scope
        .table_settab(&mut alloc, &mut builder, 1, 2, 1)
        .expect("settab 1");
    scope
        .table_settab(&mut alloc, &mut builder, 2, 3, 1)
        .expect("settab 2");
    assert_eq!((builder.array_items, builder.hash_items), (3, 2));

    scope.finish_table(builder);
    unsafe {
        let inst = *(*proto_ptr.as_ptr()).code.add(newtable_pc as usize);
        assert_eq!(inst.opcode(), Some(OpCode::NewTable));
        assert_eq!(inst.b(), 3); // min(array_items, 255)
        assert_eq!(inst.c(), newtable_hash_log2(2)); // ceil_log2(2*4/3)=ceil_log2(3)=2
    }

    // Compound assign on Local: `x += 5` -> OP_ADDI R[x], R[x], 5 (single in-place)
    scope.free_reg = 1;
    let x = scope.declare_local("x", false, false, 2).expect("x");
    scope.mark_initialized();
    let before = unsafe { (*proto_ptr.as_ptr()).code_count };
    scope
        .compile_compound_assign(
            &mut alloc,
            ExprDesc::new(ExprKind::Local {
                reg: x,
                is_const: false,
            }),
            BinOp::Add,
            ExprDesc::new(ExprKind::IntImm(5)),
            2,
        )
        .expect("x+=");
    unsafe {
        assert_eq!((*proto_ptr.as_ptr()).code_count, before + 1);
        let inst = *(*proto_ptr.as_ptr()).code.add(before as usize);
        assert_eq!(inst.opcode(), Some(OpCode::AddI));
        assert_eq!(inst.a(), x);
        assert_eq!(inst.b(), x);
        assert_eq!(inst.sc(), 5);
    }

    // Indexed compound assign: table/key evaluated once -> GETTAB, ADDI, SETTAB (3 ops)
    // R[t]=table, R[k]=key already materialized (single evaluation)
    scope.free_reg = x + 1;
    let t = scope.alloc_regs(2, 3).expect("t,k");
    let k = t + 1;
    let before_idx = unsafe { (*proto_ptr.as_ptr()).code_count };
    scope
        .compile_compound_assign(
            &mut alloc,
            ExprDesc::new(ExprKind::Indexed {
                table_reg: t,
                key_reg: k,
            }),
            BinOp::Add,
            ExprDesc::new(ExprKind::IntImm(1)),
            3,
        )
        .expect("t[k]+=");
    unsafe {
        let p = proto_ptr.as_ptr();
        assert_eq!((*p).code_count, before_idx + 3);
        let get = *(*p).code.add(before_idx as usize);
        let addi = *(*p).code.add((before_idx + 1) as usize);
        let set = *(*p).code.add((before_idx + 2) as usize);
        assert_eq!(get.opcode(), Some(OpCode::GetTab));
        assert_eq!(get.b(), t);
        assert_eq!(get.c(), k);
        assert_eq!(addi.opcode(), Some(OpCode::AddI));
        assert_eq!(set.opcode(), Some(OpCode::SetTab));
        assert_eq!(set.a(), t);
        assert_eq!(set.b(), k);
        // Same table/key registers reused — LHS evaluated exactly once
        assert_eq!(get.b(), set.a());
        assert_eq!(get.c(), set.b());
    }

    unsafe {
        proto_ptr.as_mut().free_proto(&mut alloc);
        dedup_ptr.as_mut().free_table(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn compiler_disables_op_tailcall_when_defer_is_active_in_function_scope() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let mut dedup_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("dedup");

    let mut scope = FunctionScope::new(proto_ptr.as_ptr(), dedup_ptr.as_ptr());
    scope.begin_scope();

    // Without defer: `return f()` emits OP_TAILCALL
    let f = scope.alloc_regs(1, 1).expect("f");
    scope
        .compile_return_call(&mut alloc, f, 0, 1)
        .expect("tail");
    unsafe {
        let p = proto_ptr.as_ptr();
        assert_eq!((*p).code_count, 1);
        let inst = *(*p).code;
        assert_eq!(inst.opcode(), Some(OpCode::TailCall));
        assert_eq!(inst.a(), f);
    }

    // With active defer local: MUST emit CALL + CLOSE + RETURN (never TAILCALL)
    let mut proto2 =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto2");
    let mut dedup2 = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 4).expect("dedup2");
    let mut scope2 = FunctionScope::new(proto2.as_ptr(), dedup2.as_ptr());
    scope2.begin_scope();

    let defer_reg = scope2
        .declare_local("__defer", false, true, 2)
        .expect("defer");
    scope2.mark_initialized();
    assert!(scope2.has_active_defer());
    assert_eq!(scope2.first_defer_reg(), Some(defer_reg));

    let f2 = scope2.alloc_regs(1, 2).expect("f2");
    scope2
        .compile_return_call(&mut alloc, f2, 0, 2)
        .expect("no tail");
    unsafe {
        let p = proto2.as_ptr();
        assert_eq!((*p).code_count, 3);
        let call = *(*p).code.add(0);
        let close = *(*p).code.add(1);
        let ret = *(*p).code.add(2);
        assert_eq!(call.opcode(), Some(OpCode::Call));
        assert_eq!(close.opcode(), Some(OpCode::Close));
        assert_eq!(close.a(), defer_reg);
        assert_eq!(ret.opcode(), Some(OpCode::Return));
        for pc in 0..(*p).code_count {
            assert_ne!(
                (*(*p).code.add(pc as usize)).opcode(),
                Some(OpCode::TailCall)
            );
        }
    }

    unsafe {
        proto_ptr.as_mut().free_proto(&mut alloc);
        dedup_ptr.as_mut().free_table(&mut alloc);
        proto2.as_mut().free_proto(&mut alloc);
        dedup2.as_mut().free_table(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>());
        alloc.free_bytes(proto2.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        alloc.free_bytes(dedup2.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}
