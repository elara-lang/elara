//! Intrusive in-place `OP_JMP` patch lists (`sJ` linked, zero heap allocations).

use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::proto::NO_JUMP;
use crate::compiler::scope::{CompileError, FunctionScope};
use crate::gc::GcAllocator;

impl<'a> FunctionScope<'a> {
    /// Emits `OP_JMP` whose `sJ` links to the previous pending jump at `prev` (`NO_JUMP` → `sJ = 0`).
    pub fn emit_jmp_link(
        &mut self,
        alloc: &mut GcAllocator,
        prev: u32,
        line: u32,
    ) -> Result<u32, CompileError> {
        let pc = self.pc();
        let sj = if prev == NO_JUMP {
            0
        } else {
            prev as i32 - (pc as i32 + 1)
        };
        self.emit(alloc, Instruction::make_sj(OpCode::Jmp, sj), line)
    }

    /// Returns the next pending jump PC in the intrusive list headed at `jmp_pc`, or `NO_JUMP`.
    #[must_use]
    pub fn jump_list_next(&self, jmp_pc: u32) -> u32 {
        // SAFETY: `jmp_pc` indexes an `OP_JMP` previously emitted into `self.proto`.
        let inst = unsafe { *(*self.proto).code.add(jmp_pc as usize) };
        let sj = inst.sj();
        if sj == 0 {
            NO_JUMP
        } else {
            (jmp_pc as i32 + 1 + sj) as u32
        }
    }

    /// Rewrites the intrusive list link stored in `OP_JMP` at `jmp_pc` to point at `next`.
    pub fn set_jump_link(&mut self, jmp_pc: u32, next: u32) {
        let sj = if next == NO_JUMP {
            0
        } else {
            next as i32 - (jmp_pc as i32 + 1)
        };
        // SAFETY: `jmp_pc` indexes a previously emitted instruction.
        unsafe {
            (*(*self.proto).code.add(jmp_pc as usize)).patch_sj(sj);
        }
    }

    /// Links pending jump list `l2` onto `l1` by walking `l1`'s in-place `sJ` chain (zero heap allocs).
    pub fn concat_jump_list(&mut self, l1: &mut u32, l2: u32) {
        if l2 == NO_JUMP {
            return;
        }
        if *l1 == NO_JUMP {
            *l1 = l2;
            return;
        }
        let mut pc = *l1;
        loop {
            let next = self.jump_list_next(pc);
            if next == NO_JUMP {
                self.set_jump_link(pc, l2);
                return;
            }
            pc = next;
        }
    }

    /// Patches every `OP_JMP` in `list` to jump to the current PC, recording `last_target_pc`.
    pub fn patch_jump_list_to_here(&mut self, mut list: u32) {
        let here = self.pc();
        while list != NO_JUMP {
            let next = self.jump_list_next(list);
            let offset = here as i32 - (list as i32 + 1);
            // SAFETY: `list` indexes a previously emitted `OP_JMP`.
            unsafe {
                (*(*self.proto).code.add(list as usize)).patch_sj(offset);
            }
            list = next;
        }
        if here > self.last_target_pc {
            self.last_target_pc = here;
        }
    }
}
