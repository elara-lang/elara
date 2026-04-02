//! Call-frame layout: index-based stack window (pointers refreshed after stack growth).

use crate::compiler::opcode::Instruction;
use crate::vm::closure::ObjClosure;

pub const FRAME_FLAG_PCALL: u8 = 1 << 0;
pub const FRAME_FLAG_C_BOUNDARY: u8 = 1 << 1;

/// Active function activation record. Stores `base_idx` / `func_idx` (not raw stack pointers)
/// so `ObjFiber::ensure_stack` reallocation cannot invalidate frame identity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallFrame {
    pub closure: *mut ObjClosure,
    pub pc: *const Instruction,
    pub base_idx: u32,
    pub func_idx: u32,
    pub num_results: u16,
    pub vararg_count: u16,
    pub flags: u8,
    pub reserved: [u8; 3],
}

impl CallFrame {
    /// Builds a frame for `closure` with register window starting at `base_idx`.
    ///
    /// # Safety
    ///
    /// When `closure` is non-null it must point to a live `ObjClosure` whose `proto` is valid.
    #[must_use]
    pub unsafe fn new(
        closure: *mut ObjClosure,
        base_idx: u32,
        func_idx: u32,
        num_results: u16,
    ) -> Self {
        let pc = if closure.is_null() {
            core::ptr::null()
        } else {
            // SAFETY: caller upholds the contract above.
            unsafe { (*(*closure).proto).code.cast_const() }
        };
        Self {
            closure,
            pc,
            base_idx,
            func_idx,
            num_results,
            vararg_count: 0,
            flags: 0,
            reserved: [0; 3],
        }
    }
}
