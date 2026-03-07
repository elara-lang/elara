use core::mem::size_of;
use core::ptr::NonNull;

use elara::compiler::{Instruction, ObjProto, OpCode, UpvalDesc};
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
