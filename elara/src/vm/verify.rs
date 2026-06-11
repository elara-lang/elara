//! `.elac` binary bytecode container: header, CRC32C, and recursive `ObjProto` (de)serialization.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::mem::size_of;
use core::ptr::NonNull;

use crate::compiler::opcode::Instruction;
use crate::compiler::proto::{LineRun, ObjProto, UpvalDesc};
use crate::value::nan::{ELARA_INT48_MAX, ELARA_INT48_MIN, ObjBoxedI64, ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::crc32c::crc32c;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub const ELAC_HEADER_SIZE: usize = 32;
pub const ELAC_MAGIC: [u8; 6] = [0x1B, b'E', b'L', b'A', b'R', b'A'];
pub const ELAC_VERSION_MAJOR: u8 = 1;
pub const ELAC_VERSION_MINOR: u8 = 0;
pub const ELAC_ENDIAN_LE: u8 = 1;
pub const ELAC_PTR_SIZE: u8 = 8;

const TAG_NIL: u8 = 0x00;
const TAG_BOOL: u8 = 0x01;
const TAG_INT48: u8 = 0x02;
const TAG_FLOAT64: u8 = 0x03;
const TAG_STRING: u8 = 0x04;
const TAG_BOXEDI64: u8 = 0x05;

/// Serializes `root` into a complete `.elac` buffer (32-byte header + payload).
pub unsafe fn serialize_elac(root: *const ObjProto, flags: u16) -> Result<Vec<u8>, &'static str> {
    if root.is_null() {
        return Err("null proto");
    }
    let mut payload = Vec::new();
    write_proto(&mut payload, &*root)?;
    let crc = crc32c(&payload);
    let payload_len = u32::try_from(payload.len()).map_err(|_| "payload too large")?;

    let mut out = Vec::with_capacity(ELAC_HEADER_SIZE + payload.len());
    out.extend_from_slice(&ELAC_MAGIC);
    out.push(ELAC_VERSION_MAJOR);
    out.push(ELAC_VERSION_MINOR);
    out.push(ELAC_ENDIAN_LE);
    out.push(ELAC_PTR_SIZE);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    debug_assert_eq!(out.len(), ELAC_HEADER_SIZE);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Validates the `.elac` header + CRC32C and deserializes the root `ObjProto` tree.
pub unsafe fn deserialize_elac(
    state: &mut ElaraState,
    buf: &[u8],
) -> Result<*mut ObjProto, ElaraStatus> {
    if buf.len() < ELAC_HEADER_SIZE {
        state.last_error = Some("elac: truncated header");
        return Err(ElaraStatus::ErrVerify);
    }
    if buf[0..6] != ELAC_MAGIC {
        state.last_error = Some("elac: bad magic");
        return Err(ElaraStatus::ErrVerify);
    }
    if buf[6] != ELAC_VERSION_MAJOR || buf[7] != ELAC_VERSION_MINOR {
        state.last_error = Some("elac: unsupported version");
        return Err(ElaraStatus::ErrVerify);
    }
    if buf[8] != ELAC_ENDIAN_LE || buf[9] != ELAC_PTR_SIZE {
        state.last_error = Some("elac: incompatible platform");
        return Err(ElaraStatus::ErrVerify);
    }
    let payload_len = u32::from_le_bytes(buf[0x0C..0x10].try_into().unwrap()) as usize;
    let expect_crc = u32::from_le_bytes(buf[0x10..0x14].try_into().unwrap());
    if buf.len() < ELAC_HEADER_SIZE + payload_len {
        state.last_error = Some("elac: truncated payload");
        return Err(ElaraStatus::ErrVerify);
    }
    let payload = &buf[ELAC_HEADER_SIZE..ELAC_HEADER_SIZE + payload_len];
    if crc32c(payload) != expect_crc {
        state.last_error = Some("elac: crc32c mismatch");
        return Err(ElaraStatus::ErrVerify);
    }
    let mut cursor = 0usize;
    let proto = read_proto(state, payload, &mut cursor)?;
    if let Err(msg) = crate::vm::verify_check::verify_proto(&*proto, None) {
        state.last_error = Some(msg);
        return Err(ElaraStatus::ErrVerify);
    }
    Ok(proto)
}

unsafe fn write_proto(out: &mut Vec<u8>, p: &ObjProto) -> Result<(), &'static str> {
    // source_name
    if p.source_name.is_null() {
        write_u32(out, 0);
    } else {
        let s = &*p.source_name;
        let bytes = s.as_bytes();
        write_u32(out, bytes.len() as u32);
        out.extend_from_slice(bytes);
    }
    out.push(p.num_params);
    out.push(p.is_vararg);
    out.push(p.max_stack_size);
    out.push(p.upvalue_count);

    if p.code_count == 0 {
        return Err("empty code stream");
    }
    write_u32(out, p.code_count);
    for i in 0..p.code_count as usize {
        write_u32(out, (*p.code.add(i)).0);
    }

    write_u16(out, p.const_count);
    for i in 0..p.const_count as usize {
        write_constant(out, *p.constants.add(i))?;
    }

    for i in 0..p.upvalue_count as usize {
        let u = *p.upvalues.add(i);
        out.push(u.index);
        out.push(u.is_local);
        out.push(u.is_const);
        out.push(u.reserved);
    }

    write_u32(out, p.line_run_count);
    for i in 0..p.line_run_count as usize {
        let r = *p.line_runs.add(i);
        write_u32(out, r.line);
        write_u32(out, r.count);
    }

    write_u16(out, p.proto_count);
    for i in 0..p.proto_count as usize {
        let child = *p.protos.add(i);
        if child.is_null() {
            return Err("null child proto");
        }
        write_proto(out, &*child)?;
    }
    Ok(())
}

unsafe fn write_constant(out: &mut Vec<u8>, v: Value) -> Result<(), &'static str> {
    if v.is_nil() {
        out.push(TAG_NIL);
    } else if v.is_bool() {
        out.push(TAG_BOOL);
        out.push(u8::from(v.as_bool()));
    } else if v.is_i48() {
        out.push(TAG_INT48);
        out.extend_from_slice(&v.as_i48().to_le_bytes());
    } else if v.is_float() {
        out.push(TAG_FLOAT64);
        out.extend_from_slice(&v.as_f64().to_le_bytes());
    } else if v.is_obj_type(ObjType::String) {
        out.push(TAG_STRING);
        let s = &*v.as_obj().cast::<ObjString>();
        let bytes = s.as_bytes();
        write_u32(out, bytes.len() as u32);
        out.extend_from_slice(bytes);
    } else if v.is_obj_type(ObjType::BoxedI64) {
        out.push(TAG_BOXEDI64);
        let i = (*v.as_obj().cast::<ObjBoxedI64>()).value;
        out.extend_from_slice(&i.to_le_bytes());
    } else {
        return Err("unsupported constant type");
    }
    Ok(())
}

unsafe fn read_proto(
    state: &mut ElaraState,
    buf: &[u8],
    cur: &mut usize,
) -> Result<*mut ObjProto, ElaraStatus> {
    let name_len = read_u32(buf, cur)? as usize;
    let source_name = if name_len == 0 {
        core::ptr::null_mut()
    } else {
        let bytes = read_bytes(buf, cur, name_len)?;
        state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, bytes)
            .ok_or(ElaraStatus::ErrOom)?
            .as_ptr()
    };

    let proto = ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, source_name)
        .ok_or(ElaraStatus::ErrOom)?;
    let p = proto.as_ptr();

    (*p).num_params = read_u8(buf, cur)?;
    (*p).is_vararg = read_u8(buf, cur)?;
    (*p).max_stack_size = read_u8(buf, cur)?;
    let upvalue_count = read_u8(buf, cur)?;

    let code_count = read_u32(buf, cur)?;
    if code_count == 0 {
        state.last_error = Some("elac: empty code");
        return Err(ElaraStatus::ErrVerify);
    }
    (*p).grow_code_and_ic(&mut state.alloc, code_count)
        .ok_or(ElaraStatus::ErrOom)?;
    for i in 0..code_count as usize {
        let w = read_u32(buf, cur)?;
        *((*p).code.add(i)) = Instruction(w);
        *((*p).ic_slots.add(i)) = u16::MAX;
    }
    (*p).code_count = code_count;

    let const_count = read_u16(buf, cur)?;
    let mut dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    for _ in 0..const_count {
        let v = read_constant(state, buf, cur)?;
        (*p).add_constant(&mut state.alloc, dedup.as_mut(), v)
            .map_err(|_| ElaraStatus::ErrOom)?;
    }

    for _ in 0..upvalue_count {
        let desc = UpvalDesc {
            index: read_u8(buf, cur)?,
            is_local: read_u8(buf, cur)?,
            is_const: read_u8(buf, cur)?,
            reserved: read_u8(buf, cur)?,
        };
        (*p).add_upvalue(&mut state.alloc, desc)
            .map_err(|_| ElaraStatus::ErrOom)?;
    }

    let line_run_count = read_u32(buf, cur)?;
    if line_run_count > 0 {
        let bytes = (line_run_count as usize)
            .checked_mul(size_of::<LineRun>())
            .ok_or(ElaraStatus::ErrOom)?;
        let raw = state.alloc.alloc_bytes(bytes).ok_or(ElaraStatus::ErrOom)?;
        (*p).line_runs = raw.as_ptr().cast();
        (*p).line_run_capacity = line_run_count;
        (*p).line_run_count = line_run_count;
        for i in 0..line_run_count as usize {
            let line = read_u32(buf, cur)?;
            let count = read_u32(buf, cur)?;
            *(*p).line_runs.add(i) = LineRun { line, count };
        }
    }

    let proto_count = read_u16(buf, cur)?;
    for _ in 0..proto_count {
        let child = read_proto(state, buf, cur)?;
        (*p).add_child_proto(&mut state.alloc, child)
            .map_err(|_| ElaraStatus::ErrOom)?;
    }

    // Drop temporary dedup table from the object list later via GC; unlink now to avoid leaks in tests.
    unlink_object(&mut state.all_objects, dedup.as_ptr().cast());
    dedup.as_mut().free_table(&mut state.alloc);
    state
        .alloc
        .free_bytes(dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());

    Ok(p)
}

unsafe fn read_constant(
    state: &mut ElaraState,
    buf: &[u8],
    cur: &mut usize,
) -> Result<Value, ElaraStatus> {
    match read_u8(buf, cur)? {
        TAG_NIL => Ok(Value::NIL),
        TAG_BOOL => Ok(Value::from_bool(read_u8(buf, cur)? != 0)),
        TAG_INT48 => {
            let i = i64::from_le_bytes(read_array::<8>(buf, cur)?);
            if !(ELARA_INT48_MIN..=ELARA_INT48_MAX).contains(&i) {
                state.last_error = Some("elac: int48 out of range");
                return Err(ElaraStatus::ErrVerify);
            }
            Ok(Value::from_i48(i))
        }
        TAG_FLOAT64 => {
            let bits = u64::from_le_bytes(read_array::<8>(buf, cur)?);
            Ok(Value::from_f64(f64::from_bits(bits)))
        }
        TAG_STRING => {
            let len = read_u32(buf, cur)? as usize;
            let bytes = read_bytes(buf, cur, len)?;
            let s = state
                .strings
                .new_string(&mut state.alloc, &mut state.all_objects, 0, bytes)
                .ok_or(ElaraStatus::ErrOom)?;
            Ok(Value::from_obj(s.cast(), ObjType::String))
        }
        TAG_BOXEDI64 => {
            let i = i64::from_le_bytes(read_array::<8>(buf, cur)?);
            let raw = state
                .alloc
                .alloc_bytes(size_of::<ObjBoxedI64>())
                .ok_or(ElaraStatus::ErrOom)?;
            let ptr = raw.as_ptr().cast::<ObjBoxedI64>();
            core::ptr::write(ptr, ObjBoxedI64::new(i, 0));
            (*ptr).header.next = state.all_objects;
            state.all_objects = ptr.cast();
            Ok(Value::from_obj(
                NonNull::new(ptr.cast()).unwrap(),
                ObjType::BoxedI64,
            ))
        }
        _ => {
            state.last_error = Some("elac: bad const tag");
            Err(ElaraStatus::ErrVerify)
        }
    }
}

fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn read_u8(buf: &[u8], cur: &mut usize) -> Result<u8, ElaraStatus> {
    if *cur >= buf.len() {
        return Err(ElaraStatus::ErrVerify);
    }
    let v = buf[*cur];
    *cur += 1;
    Ok(v)
}

fn read_u16(buf: &[u8], cur: &mut usize) -> Result<u16, ElaraStatus> {
    let a = read_array::<2>(buf, cur)?;
    Ok(u16::from_le_bytes(a))
}

fn read_u32(buf: &[u8], cur: &mut usize) -> Result<u32, ElaraStatus> {
    let a = read_array::<4>(buf, cur)?;
    Ok(u32::from_le_bytes(a))
}

fn read_array<const N: usize>(buf: &[u8], cur: &mut usize) -> Result<[u8; N], ElaraStatus> {
    if *cur + N > buf.len() {
        return Err(ElaraStatus::ErrVerify);
    }
    let mut a = [0u8; N];
    a.copy_from_slice(&buf[*cur..*cur + N]);
    *cur += N;
    Ok(a)
}

fn read_bytes<'a>(buf: &'a [u8], cur: &mut usize, len: usize) -> Result<&'a [u8], ElaraStatus> {
    if *cur + len > buf.len() {
        return Err(ElaraStatus::ErrVerify);
    }
    let s = &buf[*cur..*cur + len];
    *cur += len;
    Ok(s)
}

unsafe fn unlink_object(
    head: &mut *mut crate::value::GcHeader,
    target: *mut crate::value::GcHeader,
) {
    let mut cursor = head;
    while !(*cursor).is_null() {
        if *cursor == target {
            *cursor = (*target).next;
            return;
        }
        cursor = &mut (*(*cursor)).next;
    }
}
