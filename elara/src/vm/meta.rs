//! Metatable lookup and metamethod call preparation.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::too_many_arguments)]

use crate::value::{ObjTable, ObjType, Value};
use crate::vm::call::RegWindow;
use crate::vm::closure::ObjClosure;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Returns the object's metatable pointer, or null when absent / non-table.
#[inline]
#[must_use]
pub fn get_metatable(obj: Value) -> *mut ObjTable {
    if !obj.is_obj_type(ObjType::Table) {
        return core::ptr::null_mut();
    }
    // SAFETY: tag verified as `ObjType::Table`.
    unsafe { (*obj.as_obj().cast::<ObjTable>()).metatable }
}

/// Looks up `mm_key` on `obj`'s metatable (raw get, no further metamethod recursion).
#[inline]
#[must_use]
pub fn get_metamethod(obj: Value, mm_key: Value) -> Value {
    let mt = get_metatable(obj);
    if mt.is_null() {
        return Value::NIL;
    }
    // SAFETY: `mt` is a live table.
    unsafe { (*mt).get(mm_key) }
}

/// Outcome of a metamethod that may either finish inline or enter a Lua call.
pub enum MmOutcome {
    Value(Value),
    Call(RegWindow),
}

/// Resolves `__index`: table fallback or prepares a call `mm(obj, key)` into `R[dest]`.
pub unsafe fn meta_index(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    dest: u8,
    obj: Value,
    key: Value,
) -> Result<MmOutcome, ElaraStatus> {
    let mm = get_metamethod(obj, state.mm_index);
    if mm.is_nil() {
        return Ok(MmOutcome::Value(Value::NIL));
    }
    if mm.is_obj_type(ObjType::Table) {
        // SAFETY: tag verified.
        let v = unsafe { (*mm.as_obj().cast::<ObjTable>()).get(key) };
        return Ok(MmOutcome::Value(v));
    }
    if mm.is_obj_type(ObjType::Closure) {
        let w =
            unsafe { prepare_mm_call(state, fiber, frame, base, pc, dest, mm, &[obj, key], 1)? };
        return Ok(MmOutcome::Call(w));
    }
    Err(ElaraStatus::ErrRuntime)
}

/// Result of attempting `__newindex`.
pub enum NewIndexOutcome {
    /// Metamethod handled the write (table mm or no-result call finished setup).
    Done,
    /// Need to enter a metamethod call frame.
    Call(RegWindow),
    /// No `__newindex` — caller should raw-set.
    Fallback,
}

/// Resolves `__newindex`: table write fallback or call `mm(obj, key, val)`.
pub unsafe fn meta_newindex(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    dest: u8,
    obj: Value,
    key: Value,
    val: Value,
) -> Result<NewIndexOutcome, ElaraStatus> {
    let mm = get_metamethod(obj, state.mm_newindex);
    if mm.is_nil() {
        return Ok(NewIndexOutcome::Fallback);
    }
    if mm.is_obj_type(ObjType::Table) {
        // SAFETY: tag verified.
        unsafe {
            (*mm.as_obj().cast::<ObjTable>())
                .set(&mut state.alloc, key, val)
                .map_err(|_| ElaraStatus::ErrRuntime)?;
        }
        return Ok(NewIndexOutcome::Done);
    }
    if mm.is_obj_type(ObjType::Closure) {
        let w = unsafe {
            prepare_mm_call(state, fiber, frame, base, pc, dest, mm, &[obj, key, val], 0)?
        };
        return Ok(NewIndexOutcome::Call(w));
    }
    Err(ElaraStatus::ErrRuntime)
}

/// Resolves `__add` via call `mm(lhs, rhs)` storing one result at `R[dest]`.
pub unsafe fn meta_add(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    dest: u8,
    lhs: Value,
    rhs: Value,
) -> Result<RegWindow, ElaraStatus> {
    let mm = {
        let mut m = get_metamethod(lhs, state.mm_add);
        if m.is_nil() {
            m = get_metamethod(rhs, state.mm_add);
        }
        m
    };
    if !mm.is_obj_type(ObjType::Closure) {
        return Err(ElaraStatus::ErrRuntime);
    }
    unsafe { prepare_mm_call(state, fiber, frame, base, pc, dest, mm, &[lhs, rhs], 1) }
}

/// Resolves `__call`: rearranges `R[a..]` so `mm` is callable with `obj` as first argument.
pub unsafe fn try_call_metamethod(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    a: u8,
    num_args: u32,
    num_results: u16,
) -> Result<RegWindow, ElaraStatus> {
    let obj = unsafe { *base.add(a as usize) };
    let mm = get_metamethod(obj, state.mm_call);
    if !mm.is_obj_type(ObjType::Closure) {
        return Err(ElaraStatus::ErrRuntime);
    }

    // Shift existing args right by one and insert `obj` as arg0.
    unsafe {
        if num_args > 0 {
            let src = base.add(a as usize + 1);
            let dst = base.add(a as usize + 2);
            core::ptr::copy(src, dst, num_args as usize);
        }
        *base.add(a as usize) = mm;
        *base.add(a as usize + 1) = obj;
    }
    push_closure_call(
        state,
        fiber,
        frame,
        base,
        pc,
        a,
        num_args + 1,
        num_results,
        mm.as_obj().cast::<ObjClosure>(),
    )
}

/// Places `mm` and `args` at `R[dest..]` and pushes a call frame expecting `nresults`.
pub unsafe fn prepare_mm_call(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    dest: u8,
    mm: Value,
    args: &[Value],
    nresults: u16,
) -> Result<RegWindow, ElaraStatus> {
    let closure = mm.as_obj().cast::<ObjClosure>();
    unsafe {
        *base.add(dest as usize) = mm;
        for (i, arg) in args.iter().enumerate() {
            *base.add(dest as usize + 1 + i) = *arg;
        }
    }
    push_closure_call(
        state,
        fiber,
        frame,
        base,
        pc,
        dest,
        args.len() as u32,
        nresults,
        closure,
    )
}

#[allow(clippy::too_many_arguments)]
unsafe fn push_closure_call(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    _base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    a: u8,
    num_args: u32,
    num_results: u16,
    closure: *mut ObjClosure,
) -> Result<RegWindow, ElaraStatus> {
    let proto = unsafe { &*(*closure).proto };
    let func_idx = frame.base_idx + u32::from(a);
    let new_base = func_idx + 1;

    for i in num_args..u32::from(proto.num_params) {
        unsafe {
            *fiber.stack.add((new_base + i) as usize) = Value::NIL;
        }
    }

    let need = new_base + u32::from(proto.max_stack_size);
    fiber.open_top = fiber.open_top.max(need);
    fiber.ensure_stack(&mut state.alloc, need)?;

    frame.pc = pc;
    let new_frame = CallFrame {
        closure,
        pc: proto.code.cast_const(),
        base_idx: new_base,
        func_idx,
        num_results,
        vararg_count: num_args.saturating_sub(u32::from(proto.num_params)) as u16,
        flags: 0,
        reserved: [0; 3],
    };
    fiber.push_frame(&mut state.alloc, new_frame)?;

    let frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
    Ok(RegWindow {
        base: unsafe { fiber.stack.add(frame.base_idx as usize) },
        k: unsafe { (*(*frame.closure).proto).constants.cast_const() },
        pc: frame.pc,
    })
}

/// Shared by `OP_CALL` when the callable is not a closure.
pub unsafe fn call_or_metamethod(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    a: u8,
    num_args: u32,
    num_results: u16,
) -> Result<RegWindow, ElaraStatus> {
    let callable = unsafe { *base.add(a as usize) };
    if callable.is_obj_type(ObjType::Closure) {
        return push_closure_call(
            state,
            fiber,
            frame,
            base,
            pc,
            a,
            num_args,
            num_results,
            callable.as_obj().cast(),
        );
    }
    if callable.is_obj_type(ObjType::NativeFn) {
        return call_native(state, fiber, frame, base, pc, a, num_args, num_results);
    }
    unsafe { try_call_metamethod(state, fiber, frame, base, pc, a, num_args, num_results) }
}

unsafe fn call_native(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const crate::compiler::opcode::Instruction,
    a: u8,
    num_args: u32,
    num_results: u16,
) -> Result<RegWindow, ElaraStatus> {
    frame.pc = pc;
    let native = (*base.add(a as usize))
        .as_obj()
        .cast::<crate::ffi::ObjNativeFn>();
    let argc = num_args.min(u32::from(u16::MAX)) as u16;
    let args_ptr = base.add(a as usize + 1);
    // Results overwrite the callable slot.
    let max_results = if num_results == 0xFFFF {
        16u16
    } else {
        num_results
    };
    let mut scratch = [Value::NIL; 16];
    let n_scratch = max_results.min(16) as usize;
    let mut ctx = crate::ffi::CallContext {
        argc,
        max_results: n_scratch as u16,
        nresults: 0,
        reserved: 0,
        args: args_ptr,
        results: scratch.as_mut_ptr(),
        userdata: (*native).userdata,
    };
    let st = ((*native).func)(state as *mut ElaraState, &mut ctx);
    if st != ElaraStatus::Ok {
        return Err(st);
    }
    let n_out = if num_results == 0xFFFF {
        ctx.nresults as u32
    } else {
        u32::from(num_results)
    };
    for (i, slot) in scratch.iter().enumerate().take(n_out as usize) {
        let v = if i < ctx.nresults as usize {
            *slot
        } else {
            Value::NIL
        };
        *base.add(a as usize + i) = v;
    }
    fiber.open_top = frame.base_idx + u32::from(a) + n_out;
    Ok(RegWindow {
        base: fiber.stack.add(frame.base_idx as usize),
        k: (*(*frame.closure).proto).constants.cast_const(),
        pc: frame.pc,
    })
}
