use core::mem::size_of;

use elara::compiler::{
    ELARA_MAX_BX, ELARA_OPCODE_COUNT, ELARA_SBX_MAX, ELARA_SBX_MIN, ELARA_SJ_MAX, ELARA_SJ_MIN,
    InstrFormat, Instruction, ObjProto, OpCode, decode_rk, disassemble_instruction,
    disassemble_proto, encode_rk_const,
};
use elara::gc::GcAllocator;
use elara::value::GcHeader;

#[test]
fn instruction_methods_encode_and_decode_iabc_iabx_iasbx_and_isj() {
    assert_eq!(ELARA_OPCODE_COUNT, 57);
    assert_eq!(OpCode::LeI as u8, 0x38);
    assert_eq!(OpCode::from_u8(0x39), None);
    assert_eq!(OpCode::from_u8(0x12), Some(OpCode::Add));

    // iABC: ADD R[2], R[0], RK(K[5])
    let rk = encode_rk_const(5);
    assert_eq!(rk, 0x85);
    assert_eq!(decode_rk(rk), Err(5));
    assert_eq!(decode_rk(7), Ok(7));

    let add = Instruction::make_abc(OpCode::Add, 2, 0, rk);
    assert_eq!(add.opcode(), Some(OpCode::Add));
    assert_eq!(add.opcode_raw(), 0x12);
    assert_eq!((add.a(), add.b(), add.c()), (2, 0, rk));
    assert!(OpCode::Add.uses_rk_c());
    assert_eq!(OpCode::Add.format(), InstrFormat::Abc);
    assert_eq!(OpCode::Add.name(), "ADD");

    // iABx: LOADK R[1], K[1000]
    let loadk = Instruction::make_abx(OpCode::LoadK, 1, 1000);
    assert_eq!(loadk.opcode(), Some(OpCode::LoadK));
    assert_eq!(loadk.a(), 1);
    assert_eq!(loadk.bx(), 1000);
    assert_eq!(OpCode::LoadK.format(), InstrFormat::Abx);

    // iAsBx: LOADI R[3], -42
    let loadi = Instruction::make_asbx(OpCode::LoadI, 3, -42);
    assert_eq!(loadi.opcode(), Some(OpCode::LoadI));
    assert_eq!(loadi.a(), 3);
    assert_eq!(loadi.sbx(), -42);
    assert_eq!(OpCode::LoadI.format(), InstrFormat::Asbx);

    // isJ: JMP +10
    let jmp = Instruction::make_sj(OpCode::Jmp, 10);
    assert_eq!(jmp.opcode(), Some(OpCode::Jmp));
    assert_eq!(jmp.sj(), 10);
    assert_eq!(OpCode::Jmp.format(), InstrFormat::Sj);
    assert_eq!(OpCode::Jmp.name(), "JMP");

    // sC immediate on ADDI
    let addi = Instruction::make_abc(OpCode::AddI, 1, 1, (-5i8) as u8);
    assert_eq!(addi.sc(), -5);
    assert!(OpCode::AddI.uses_sc());
    assert!(!OpCode::AddI.uses_rk_c());

    // Patch helpers preserve opcode / unrelated fields
    let mut inst = Instruction::make_abc(OpCode::NewTable, 0, 0, 0);
    inst.patch_a(4);
    inst.patch_b_c(8, 3);
    assert_eq!(inst.opcode(), Some(OpCode::NewTable));
    assert_eq!((inst.a(), inst.b(), inst.c()), (4, 8, 3));

    let mut k = Instruction::make_abx(OpCode::LoadK, 0, 0);
    k.patch_bx(ELARA_MAX_BX);
    assert_eq!(k.bx(), ELARA_MAX_BX);

    let mut s = Instruction::make_asbx(OpCode::ForPrep, 2, 0);
    s.patch_sbx(100);
    assert_eq!(s.sbx(), 100);
    assert_eq!(s.a(), 2);

    let mut j = Instruction::make_sj(OpCode::Jmp, 0);
    j.patch_sj(-3);
    assert_eq!(j.sj(), -3);

    // Format taxonomy spot-checks across all 57 opcodes
    for raw in 0u8..=0x38 {
        let op = OpCode::from_u8(raw).expect("contiguous ISA");
        assert_eq!(op as u8, raw);
        assert!(!op.name().is_empty());
        let _ = op.format();
    }
}

#[test]
fn signed_offsets_sbx_and_sj_round_trip_at_boundaries() {
    assert_eq!(ELARA_SBX_MIN, -32_767);
    assert_eq!(ELARA_SBX_MAX, 32_768);
    assert_eq!(ELARA_SJ_MIN, -8_388_607);
    assert_eq!(ELARA_SJ_MAX, 8_388_608);

    for &sbx in &[
        ELARA_SBX_MIN,
        ELARA_SBX_MIN + 1,
        -1,
        0,
        1,
        ELARA_SBX_MAX - 1,
        ELARA_SBX_MAX,
    ] {
        let inst = Instruction::make_asbx(OpCode::LoadI, 7, sbx);
        assert_eq!(inst.sbx(), sbx, "sBx round-trip failed for {sbx}");
        assert_eq!(inst.a(), 7);
        assert_eq!(inst.opcode(), Some(OpCode::LoadI));

        let mut patched = Instruction::make_asbx(OpCode::ForLoop, 1, 0);
        patched.patch_sbx(sbx);
        assert_eq!(patched.sbx(), sbx);
    }

    for &sj in &[
        ELARA_SJ_MIN,
        ELARA_SJ_MIN + 1,
        -1,
        0,
        1,
        ELARA_SJ_MAX - 1,
        ELARA_SJ_MAX,
    ] {
        let inst = Instruction::make_sj(OpCode::Jmp, sj);
        assert_eq!(inst.sj(), sj, "sJ round-trip failed for {sj}");
        assert_eq!(inst.opcode(), Some(OpCode::Jmp));

        let mut patched = Instruction::make_sj(OpCode::Jmp, 0);
        patched.patch_sj(sj);
        assert_eq!(patched.sj(), sj);
    }
}

#[test]
fn disassembler_formats_all_57_opcodes_with_operands_and_lines() {
    // Every opcode mnemonic appears when formatting a representative instruction.
    for raw in 0u8..=0x38 {
        let op = OpCode::from_u8(raw).expect("opcode");
        let inst = match op.format() {
            InstrFormat::Abc => Instruction::make_abc(op, 1, 2, encode_rk_const(3)),
            InstrFormat::Abx => Instruction::make_abx(op, 1, 42),
            InstrFormat::Asbx => Instruction::make_asbx(op, 1, -5),
            InstrFormat::Sj => Instruction::make_sj(op, 7),
        };
        let line = disassemble_instruction(inst, raw as u32, u32::from(raw) + 1);
        assert!(
            line.contains(op.name()),
            "missing mnemonic {} in `{line}`",
            op.name()
        );
        assert!(
            line.contains(&format!("{raw:04}")),
            "missing pc in `{line}`"
        );
        assert!(
            line.contains(&format!("[{:5}]", u32::from(raw) + 1)),
            "missing line annotation in `{line}`"
        );
    }

    // RK / sC / jump target annotations
    let add = Instruction::make_abc(OpCode::Add, 2, 0, encode_rk_const(5));
    let add_s = disassemble_instruction(add, 0, 10);
    assert!(add_s.contains("ADD"));
    assert!(add_s.contains("R[2]"));
    assert!(add_s.contains("K[5]"), "{add_s}");

    let addi = Instruction::make_abc(OpCode::AddI, 1, 1, (-3i8) as u8);
    let addi_s = disassemble_instruction(addi, 1, 11);
    assert!(addi_s.contains("sC=-3"), "{addi_s}");

    let jmp = Instruction::make_sj(OpCode::Jmp, 4);
    let jmp_s = disassemble_instruction(jmp, 2, 12);
    assert!(jmp_s.contains("-> 7"), "{jmp_s}"); // pc+1+sJ = 2+1+4

    // Proto dump includes line RLE and every emitted mnemonic
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();
    let mut proto_ptr =
        ObjProto::new(&mut alloc, &mut all_objects, 0, core::ptr::null_mut()).expect("proto");
    let proto = unsafe { proto_ptr.as_mut() };

    let samples = [
        (Instruction::make_asbx(OpCode::LoadI, 0, 1), 3),
        (Instruction::make_asbx(OpCode::LoadI, 1, 2), 3),
        (
            Instruction::make_abc(OpCode::Add, 2, 0, encode_rk_const(0)),
            4,
        ),
        (Instruction::make_sj(OpCode::Jmp, 1), 4),
        (Instruction::make_abc(OpCode::Return, 2, 2, 0), 5),
    ];
    for (inst, line) in samples {
        proto
            .emit_instruction(&mut alloc, inst, line)
            .expect("emit");
    }

    let dump = disassemble_proto(proto);
    assert!(dump.contains("LOADI"), "{dump}");
    assert!(dump.contains("ADD"), "{dump}");
    assert!(dump.contains("JMP"), "{dump}");
    assert!(dump.contains("RETURN"), "{dump}");
    assert!(dump.contains("[    3]"), "{dump}");
    assert!(dump.contains("[    4]"), "{dump}");
    assert!(dump.contains("[    5]"), "{dump}");
    assert_eq!(dump.lines().filter(|l| l.starts_with("000")).count(), 5);

    unsafe {
        proto.free_proto(&mut alloc);
        alloc.free_bytes(proto_ptr.as_ptr().cast::<u8>(), size_of::<ObjProto>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}
