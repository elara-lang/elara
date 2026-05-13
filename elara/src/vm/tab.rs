//! Table access, inline cache, length, and numeric/generic for-loop helpers.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::too_many_arguments)]

use crate::compiler::opcode::Instruction;
use crate::value::{ObjString, ObjTable, ObjType, Value};
use crate::vm::call::RegWindow;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::meta::{MmOutcome, NewIndexOutcome, meta_index, meta_newindex};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// `OP_NEWTABLE A, B, C` — array hint `B`, hash capacity `2^C` (0 when `C == 0`).
pub unsafe fn op_newtable(
    state: &mut ElaraState,
    base: *mut Value,
    a: u8,
    b: u8,
    c: u8,
) -> Result<(), ElaraStatus> {
    let hash_cap = if c == 0 { 0 } else { 1u32 << c };
    let t = ObjTable::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        1,
        u32::from(b),
        hash_cap,
    )
    .ok_or(ElaraStatus::ErrOom)?;
    unsafe {
        *base.add(a as usize) = Value::from_obj(t.cast(), ObjType::Table);
    }
    Ok(())
}

/// `OP_GETTAB` / shared table read with `__index` fallback.
pub unsafe fn op_gettab(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const Instruction,
    a: u8,
    table: Value,
    key: Value,
) -> Result<Option<RegWindow>, ElaraStatus> {
    if table.is_obj_type(ObjType::Table) {
        let t = unsafe { &*table.as_obj().cast::<ObjTable>() };
        let v = t.get(key);
        if !v.is_nil() {
            unsafe {
                *base.add(a as usize) = v;
            }
            return Ok(None);
        }
    }
    match unsafe { meta_index(state, fiber, frame, base, pc, a, table, key)? } {
        MmOutcome::Value(v) => {
            unsafe {
                *base.add(a as usize) = v;
            }
            Ok(None)
        }
        MmOutcome::Call(w) => Ok(Some(w)),
    }
}

/// `OP_SETTAB` with `__newindex` when the key is absent.
pub unsafe fn op_settab(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const Instruction,
    scratch: u8,
    table: Value,
    key: Value,
    val: Value,
) -> Result<Option<RegWindow>, ElaraStatus> {
    if !table.is_obj_type(ObjType::Table) {
        return match unsafe {
            meta_newindex(state, fiber, frame, base, pc, scratch, table, key, val)?
        } {
            NewIndexOutcome::Call(w) => Ok(Some(w)),
            NewIndexOutcome::Done => Ok(None),
            NewIndexOutcome::Fallback => Err(ElaraStatus::ErrRuntime),
        };
    }
    let t = unsafe { &mut *table.as_obj().cast::<ObjTable>() };
    let existing = t.get(key);
    if existing.is_nil() {
        match unsafe { meta_newindex(state, fiber, frame, base, pc, scratch, table, key, val)? } {
            NewIndexOutcome::Call(w) => return Ok(Some(w)),
            NewIndexOutcome::Done => return Ok(None),
            NewIndexOutcome::Fallback => {}
        }
    }
    t.set(&mut state.alloc, key, val)
        .map_err(|_| ElaraStatus::ErrRuntime)?;
    unsafe {
        crate::gc::barrier::barrier_table_write(state, t as *mut ObjTable, key, val);
    }
    Ok(None)
}

/// `OP_GETFIELD` with 1-slot inline cache when the table has no metatable.
pub unsafe fn op_getfield(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    k: *const Value,
    ic: *mut u16,
    pc: *const Instruction,
    inst: Instruction,
) -> Result<Option<RegWindow>, ElaraStatus> {
    let a = inst.a();
    let table = unsafe { *base.add(inst.b() as usize) };
    let key = unsafe { *k.add(inst.c() as usize) };
    let pc_idx = unsafe { pc_index(frame, pc) };

    if table.is_obj_type(ObjType::Table) {
        let t = unsafe { &*table.as_obj().cast::<ObjTable>() };
        if t.metatable.is_null() {
            let slot = unsafe { *ic.add(pc_idx) } as u32;
            if slot <= t.hash_mask && !t.entries.is_null() {
                let entry = unsafe { &*t.entries.add(slot as usize) };
                if entry.key.0 == key.0 {
                    unsafe {
                        *base.add(a as usize) = entry.value;
                    }
                    return Ok(None);
                }
            }
            if let Some(found) = t.find_hash_index(key) {
                if found <= u32::from(u16::MAX) {
                    unsafe {
                        *ic.add(pc_idx) = found as u16;
                    }
                }
                unsafe {
                    *base.add(a as usize) = (*t.entries.add(found as usize)).value;
                }
                return Ok(None);
            }
            // Absent in hash — try array / full get then __index.
            let v = t.get(key);
            if !v.is_nil() {
                unsafe {
                    *base.add(a as usize) = v;
                }
                return Ok(None);
            }
        } else {
            let v = t.get(key);
            if !v.is_nil() {
                unsafe {
                    *base.add(a as usize) = v;
                }
                return Ok(None);
            }
        }
    }
    match unsafe { meta_index(state, fiber, frame, base, pc, a, table, key)? } {
        MmOutcome::Value(v) => {
            unsafe {
                *base.add(a as usize) = v;
            }
            Ok(None)
        }
        MmOutcome::Call(w) => Ok(Some(w)),
    }
}

/// `OP_SETFIELD A, B, C` — `R[A][K[B]] = R[C]` with inline cache.
pub unsafe fn op_setfield(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    k: *const Value,
    ic: *mut u16,
    pc: *const Instruction,
    inst: Instruction,
) -> Result<Option<RegWindow>, ElaraStatus> {
    let table = unsafe { *base.add(inst.a() as usize) };
    let key = unsafe { *k.add(inst.b() as usize) };
    let val = unsafe { *base.add(inst.c() as usize) };
    let pc_idx = unsafe { pc_index(frame, pc) };

    if table.is_obj_type(ObjType::Table) {
        let t = unsafe { &mut *table.as_obj().cast::<ObjTable>() };
        if t.metatable.is_null() {
            let slot = unsafe { *ic.add(pc_idx) } as u32;
            if slot <= t.hash_mask && !t.entries.is_null() {
                let entry = unsafe { &mut *t.entries.add(slot as usize) };
                if entry.key.0 == key.0 {
                    entry.value = val;
                    unsafe {
                        crate::gc::barrier::barrier_table_write(
                            state,
                            t as *mut ObjTable,
                            key,
                            val,
                        );
                    }
                    return Ok(None);
                }
            }
            t.set(&mut state.alloc, key, val)
                .map_err(|_| ElaraStatus::ErrRuntime)?;
            unsafe {
                crate::gc::barrier::barrier_table_write(state, t as *mut ObjTable, key, val);
            }
            if let Some(found) = t.find_hash_index(key)
                && found <= u32::from(u16::MAX)
            {
                unsafe {
                    *ic.add(pc_idx) = found as u16;
                }
            }
            return Ok(None);
        }
        if !t.get(key).is_nil() {
            t.set(&mut state.alloc, key, val)
                .map_err(|_| ElaraStatus::ErrRuntime)?;
            unsafe {
                crate::gc::barrier::barrier_table_write(state, t as *mut ObjTable, key, val);
            }
            return Ok(None);
        }
    }
    match unsafe { meta_newindex(state, fiber, frame, base, pc, inst.a(), table, key, val)? } {
        NewIndexOutcome::Call(w) => Ok(Some(w)),
        NewIndexOutcome::Done => Ok(None),
        NewIndexOutcome::Fallback => {
            if table.is_obj_type(ObjType::Table) {
                unsafe {
                    let t = table.as_obj().cast::<ObjTable>();
                    (*t).set(&mut state.alloc, key, val)
                        .map_err(|_| ElaraStatus::ErrRuntime)?;
                    crate::gc::barrier::barrier_table_write(state, t, key, val);
                }
                Ok(None)
            } else {
                Err(ElaraStatus::ErrRuntime)
            }
        }
    }
}

/// `OP_SELF A, B, C` — `R[A+1] = R[B]; R[A] = R[B][K[C]]`.
pub unsafe fn op_self(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    k: *const Value,
    ic: *mut u16,
    pc: *const Instruction,
    inst: Instruction,
) -> Result<Option<RegWindow>, ElaraStatus> {
    let a = inst.a();
    let b = inst.b();
    let table = unsafe { *base.add(b as usize) };
    unsafe {
        *base.add(a as usize + 1) = table;
    }
    // Reuse getfield path with dest=A, table at B, key K[C].
    let fake = Instruction::make_abc(crate::compiler::opcode::OpCode::GetField, a, b, inst.c());
    unsafe { op_getfield(state, fiber, frame, base, k, ic, pc, fake) }
}

/// `OP_GETI` / `OP_SETI` fast array paths.
pub unsafe fn op_geti(base: *mut Value, a: u8, b: u8, c: u8) -> Result<(), ElaraStatus> {
    let table = unsafe { *base.add(b as usize) };
    if !table.is_obj_type(ObjType::Table) {
        return Err(ElaraStatus::ErrRuntime);
    }
    let t = unsafe { &*table.as_obj().cast::<ObjTable>() };
    unsafe {
        *base.add(a as usize) = t.get_int(i64::from(c));
    }
    Ok(())
}

pub unsafe fn op_seti(
    state: &mut ElaraState,
    base: *mut Value,
    a: u8,
    b: u8,
    c: u8,
) -> Result<(), ElaraStatus> {
    let table = unsafe { *base.add(a as usize) };
    if !table.is_obj_type(ObjType::Table) {
        return Err(ElaraStatus::ErrRuntime);
    }
    let t = unsafe { &mut *table.as_obj().cast::<ObjTable>() };
    let key = Value::from_i48(i64::from(b));
    let val = unsafe { *base.add(c as usize) };
    t.set(&mut state.alloc, key, val)
        .map_err(|_| ElaraStatus::ErrRuntime)?;
    unsafe {
        crate::gc::barrier::barrier_table_write(state, t as *mut ObjTable, key, val);
    }
    Ok(())
}

/// `OP_SETLIST A, B, C` — batch `R[A][C*64 + i] = R[A+1+i]` for `i in 0..B`.
pub unsafe fn op_setlist(
    state: &mut ElaraState,
    base: *mut Value,
    a: u8,
    b: u8,
    c: u8,
) -> Result<(), ElaraStatus> {
    let table = unsafe { *base.add(a as usize) };
    if !table.is_obj_type(ObjType::Table) {
        return Err(ElaraStatus::ErrRuntime);
    }
    let t = unsafe { &mut *table.as_obj().cast::<ObjTable>() };
    let start = u32::from(c) * 64;
    for i in 0..u32::from(b) {
        let val = unsafe { *base.add(a as usize + 1 + i as usize) };
        t.set(&mut state.alloc, Value::from_i48(i64::from(start + i)), val)
            .map_err(|_| ElaraStatus::ErrRuntime)?;
    }
    Ok(())
}

/// `OP_LEN A, B`.
pub unsafe fn op_len(base: *mut Value, a: u8, b: u8) -> Result<(), ElaraStatus> {
    let v = unsafe { *base.add(b as usize) };
    let n = if v.is_obj_type(ObjType::String) {
        i64::from(unsafe { (*v.as_obj().cast::<ObjString>()).byte_len })
    } else if v.is_obj_type(ObjType::Table) {
        i64::from(unsafe { (*v.as_obj().cast::<ObjTable>()).array_len })
    } else {
        return Err(ElaraStatus::ErrRuntime);
    };
    unsafe {
        *base.add(a as usize) = Value::from_i48(n);
    }
    Ok(())
}

/// `OP_FORPREP`: `R[A] -= R[A+2]` then jump `sBx` (lands on `FORLOOP`).
pub unsafe fn op_forprep(
    base: *mut Value,
    pc: *const Instruction,
    inst: Instruction,
) -> *const Instruction {
    let a = inst.a() as usize;
    let idx = unsafe { *base.add(a) };
    let step = unsafe { *base.add(a + 2) };
    if let (Some(i), Some(s)) = (idx.to_i64(), step.to_i64()) {
        unsafe {
            *base.add(a) = Value::from_i48(i.wrapping_sub(s));
        }
    } else if let (Some(i), Some(s)) = (as_f64(idx), as_f64(step)) {
        unsafe {
            *base.add(a) = Value::from_f64(i - s);
        }
    }
    unsafe { pc.offset(inst.sbx() as isize) }
}

/// `OP_FORLOOP`: advance index; if in range set `R[A+3]` and jump `sBx`.
pub unsafe fn op_forloop(
    base: *mut Value,
    pc: *const Instruction,
    inst: Instruction,
    gas_limited: bool,
    fuel: &mut i64,
) -> Result<*const Instruction, ElaraStatus> {
    let a = inst.a() as usize;
    let idx = unsafe { *base.add(a) };
    let limit = unsafe { *base.add(a + 1) };
    let step = unsafe { *base.add(a + 2) };

    if let (Some(i), Some(lim), Some(s)) = (idx.to_i64(), limit.to_i64(), step.to_i64()) {
        let next = i.wrapping_add(s);
        let in_bounds = if s >= 0 { next < lim } else { next > lim };
        if in_bounds {
            if gas_limited {
                *fuel -= 1;
                if *fuel <= 0 {
                    return Err(ElaraStatus::ErrGas);
                }
            }
            let v = Value::from_i48(next);
            unsafe {
                *base.add(a) = v;
                *base.add(a + 3) = v;
            }
            return Ok(unsafe { pc.offset(inst.sbx() as isize) });
        }
        return Ok(pc);
    }

    let (Some(i), Some(lim), Some(s)) = (as_f64(idx), as_f64(limit), as_f64(step)) else {
        return Err(ElaraStatus::ErrRuntime);
    };
    let next = i + s;
    let in_bounds = if s >= 0.0 { next < lim } else { next > lim };
    if in_bounds {
        if gas_limited {
            *fuel -= 1;
            if *fuel <= 0 {
                return Err(ElaraStatus::ErrGas);
            }
        }
        let v = Value::from_f64(next);
        unsafe {
            *base.add(a) = v;
            *base.add(a + 3) = v;
        }
        return Ok(unsafe { pc.offset(inst.sbx() as isize) });
    }
    Ok(pc)
}

/// `OP_FORIN`: `R[A]`=table, `R[A+1]`=cursor → write key/val; jump `sBx` when exhausted.
pub unsafe fn op_forin(
    base: *mut Value,
    pc: *const Instruction,
    inst: Instruction,
) -> Result<*const Instruction, ElaraStatus> {
    let a = inst.a() as usize;
    let table = unsafe { *base.add(a) };
    if !table.is_obj_type(ObjType::Table) {
        return Err(ElaraStatus::ErrRuntime);
    }
    let cursor = unsafe { (*base.add(a + 1)).to_i64().unwrap_or(0) } as u32;
    let t = unsafe { &*table.as_obj().cast::<ObjTable>() };
    match unsafe { t.next_entry(cursor) } {
        Some((next, key, val)) => {
            unsafe {
                *base.add(a + 1) = Value::from_i48(i64::from(next));
                *base.add(a + 2) = key;
                *base.add(a + 3) = val;
            }
            Ok(pc)
        }
        None => Ok(unsafe { pc.offset(inst.sbx() as isize) }),
    }
}

/// `OP_RANGE`: `R[A] = { start=R[B], end=R[B+1], inclusive=C }` as a 3-slot array table.
pub unsafe fn op_range(
    state: &mut ElaraState,
    base: *mut Value,
    a: u8,
    b: u8,
    c: u8,
) -> Result<(), ElaraStatus> {
    let start = unsafe { *base.add(b as usize) };
    let end = unsafe { *base.add(b as usize + 1) };
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 4, 0)
        .ok_or(ElaraStatus::ErrOom)?;
    let table = unsafe { &mut *t.as_ptr() };
    table
        .set(&mut state.alloc, Value::from_i48(0), start)
        .map_err(|_| ElaraStatus::ErrOom)?;
    table
        .set(&mut state.alloc, Value::from_i48(1), end)
        .map_err(|_| ElaraStatus::ErrOom)?;
    table
        .set(
            &mut state.alloc,
            Value::from_i48(2),
            Value::from_bool(c != 0),
        )
        .map_err(|_| ElaraStatus::ErrOom)?;
    unsafe {
        *base.add(a as usize) = Value::from_obj(t.cast(), ObjType::Table);
    }
    Ok(())
}

#[inline]
unsafe fn pc_index(frame: &CallFrame, pc: *const Instruction) -> usize {
    let code = unsafe { (*(*frame.closure).proto).code.cast_const() };
    // `pc` already advanced past the current instruction.
    unsafe { pc.offset_from(code) as usize - 1 }
}

#[inline]
fn as_f64(v: Value) -> Option<f64> {
    if let Some(i) = v.to_i64() {
        Some(i as f64)
    } else if v.is_float() {
        Some(v.as_f64())
    } else {
        None
    }
}
