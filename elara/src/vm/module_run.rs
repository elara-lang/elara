//! Compile or verify a module chunk and execute it to an exports table.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ptr::NonNull;

use crate::value::nan::{ObjType, Value};
use crate::vm::closure::ObjClosure;
use crate::vm::dispatch::execute;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::module::set_module_error;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::verify::deserialize_elac;

pub unsafe fn compile_and_run_module_source(
    state: &mut ElaraState,
    chunk_name: &str,
    source: &str,
) -> Result<Value, ElaraStatus> {
    let proto = match crate::compiler::compile_module(state, chunk_name, source) {
        Ok(p) => p,
        Err(e) => {
            crate::compiler::set_compile_error(state, &e);
            return Err(ElaraStatus::ErrSyntax);
        }
    };
    if let Err(msg) = crate::vm::verify_proto(&*proto, None) {
        state.last_error = Some(msg);
        return Err(ElaraStatus::ErrVerify);
    }
    run_module_proto(state, proto)
}

pub unsafe fn load_and_run_bytecode(
    state: &mut ElaraState,
    bytes: &[u8],
) -> Result<Value, ElaraStatus> {
    let proto = deserialize_elac(state, bytes)?;
    run_module_proto(state, proto)
}

unsafe fn run_module_proto(
    state: &mut ElaraState,
    proto: *mut crate::compiler::ObjProto,
) -> Result<Value, ElaraStatus> {
    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto)
        .ok_or(ElaraStatus::ErrOom)?;
    crate::compiler::bind_env_upvalue(state, closure.as_ptr())?;
    let stack_slots = (1 + (*proto).max_stack_size as u32).max(16);
    let mut fiber = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, stack_slots, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    let f = fiber.as_mut();
    let cv = Value::from_obj(
        NonNull::new(closure.as_ptr().cast()).unwrap(),
        ObjType::Closure,
    );
    *f.stack = cv;
    f.open_top = 1;
    f.push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))?;

    let saved = state.current_fiber;
    state.current_fiber = fiber.as_ptr();
    let st = execute(state, fiber.as_ptr());
    state.current_fiber = saved;

    let out = if st == ElaraStatus::Ok {
        let v = *f.stack;
        if !v.is_obj_type(ObjType::Table) {
            set_module_error(
                state,
                format_args!("Module did not return an exports table"),
            );
            Err(ElaraStatus::ErrModule)
        } else {
            Ok(v)
        }
    } else {
        Err(st)
    };

    let fiber_ptr = fiber.as_ptr();
    unlink_object(&mut state.all_objects, fiber_ptr.cast());
    (*fiber_ptr).free_fiber(&mut state.alloc);
    out
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
