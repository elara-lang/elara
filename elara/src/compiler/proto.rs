//! Function prototype (`ObjProto`), constant pool deduplication, `ic_slots`, and RLE line table.

use core::mem::size_of;
use core::ptr::NonNull;

use crate::compiler::opcode::Instruction;
use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjString, ObjTable, ObjType, Value};

pub const ELARA_MAX_REGISTERS: usize = 255;
pub const ELARA_MAX_LOCALS: usize = 200;
pub const ELARA_MAX_UPVALUES: usize = 255;
pub const ELARA_MAX_CONSTANTS: usize = 65535;
pub const NO_JUMP: u32 = u32::MAX;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpvalDesc {
    pub index: u8,    // Register index in parent frame or upvalue index in parent
    pub is_local: u8, // 1 if capturing parent's local register, 0 if parent's upvalue
    pub is_const: u8, // 1 if the captured binding was declared `const`
    pub reserved: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRun {
    pub line: u32,  // 1-based source line number
    pub count: u32, // Number of consecutive bytecode instructions on this line
}

#[repr(C, align(8))]
pub struct ObjProto {
    pub header: GcHeader,            // ObjType::Proto
    pub code: *mut Instruction,      // Allocated via GcAllocator
    pub ic_slots: *mut u16,          // Parallel 1-slot Inline Cache array [0..code_count]
    pub constants: *mut Value,       // Constant pool K[0..const_count]
    pub protos: *mut *mut ObjProto,  // Nested child function prototypes
    pub upvalues: *mut UpvalDesc,    // Upvalue capture descriptors
    pub line_runs: *mut LineRun,     // RLE-compressed PC-to-line mapping
    pub source_name: *mut ObjString, // File or chunk name
    pub code_count: u32,
    pub code_capacity: u32,
    pub line_run_count: u32,
    pub line_run_capacity: u32,
    pub const_count: u16,
    pub const_capacity: u16,
    pub proto_count: u16,
    pub proto_capacity: u16,
    pub num_params: u8,     // Fixed parameter count
    pub is_vararg: u8,      // 1 if function accepts `...`
    pub max_stack_size: u8, // High-water mark of virtual registers (>= 2)
    pub upvalue_count: u8,
}

impl ObjProto {
    /// Allocates and initializes a new empty `ObjProto` on the GC heap.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        source_name: *mut ObjString,
    ) -> Option<NonNull<Self>> {
        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();

        // SAFETY: `ptr` is valid, 8-byte aligned, and sized for `ObjProto`.
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Proto, gc_color),
                    code: core::ptr::null_mut(),
                    ic_slots: core::ptr::null_mut(),
                    constants: core::ptr::null_mut(),
                    protos: core::ptr::null_mut(),
                    upvalues: core::ptr::null_mut(),
                    line_runs: core::ptr::null_mut(),
                    source_name,
                    code_count: 0,
                    code_capacity: 0,
                    line_run_count: 0,
                    line_run_capacity: 0,
                    const_count: 0,
                    const_capacity: 0,
                    proto_count: 0,
                    proto_capacity: 0,
                    num_params: 0,
                    is_vararg: 0,
                    max_stack_size: 2,
                    upvalue_count: 0,
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }

        NonNull::new(ptr)
    }

    /// Appends a 32-bit `Instruction` and its 1-based `line` number to the prototype, growing
    /// `code`, `ic_slots` (initialized to `0xFFFF` as empty cache sentinel), and `line_runs` (RLE).
    /// Returns the emitted instruction's PC index (`0..code_count - 1`).
    pub fn emit_instruction(
        &mut self,
        alloc: &mut GcAllocator,
        inst: Instruction,
        line: u32,
    ) -> Result<u32, &'static str> {
        if self.code_count == self.code_capacity {
            let new_cap = if self.code_capacity == 0 {
                16
            } else {
                self.code_capacity.checked_mul(2).ok_or("Out of memory")?
            };
            self.grow_code_and_ic(alloc, new_cap)
                .ok_or("Out of memory")?;
        }

        let pc = self.code_count;
        // SAFETY: `pc < self.code_capacity` in both `code` and `ic_slots`.
        unsafe {
            *self.code.add(pc as usize) = inst;
            *self.ic_slots.add(pc as usize) = u16::MAX;
        }
        self.code_count += 1;

        self.append_line_rle(alloc, line).ok_or("Out of memory")?;
        Ok(pc)
    }

    /// Adds `val` to the prototype's constant pool `K[0..const_count]`, using `dedup_map` (`ObjTable`)
    /// to deduplicate identical constants in O(1).
    ///
    /// # Errors
    ///
    /// - Returns `Err("Constant pool overflow (max 65535)")` if the pool already holds 65535 constants.
    /// - Returns `Err("Out of memory")` if buffer growth fails.
    pub fn add_constant(
        &mut self,
        alloc: &mut GcAllocator,
        dedup_map: &mut ObjTable,
        val: Value,
    ) -> Result<u16, &'static str> {
        // Lookup in `dedup_map` (only for keys valid in `ObjTable`: non-nil and non-NaN)
        let can_map = !val.is_nil() && (!val.is_float() || !val.as_f64().is_nan());
        if can_map {
            let existing = dedup_map.get(val);
            if let Some(idx) = existing.to_i64() {
                // Distinguish `-0.0` vs `+0.0` and integer `1` vs float `1.0` so type identity in K[i] is preserved!
                let k_val = unsafe { *self.constants.add(idx as usize) };
                if same_constant_rep(k_val, val) {
                    return Ok(idx as u16);
                }
            }
        } else {
            // Linear scan for `nil` or `NaN`
            for i in 0..self.const_count {
                let k_val = unsafe { *self.constants.add(i as usize) };
                if same_constant_rep(k_val, val) {
                    return Ok(i);
                }
            }
        }

        if usize::from(self.const_count) >= ELARA_MAX_CONSTANTS {
            return Err("Constant pool overflow (max 65535)");
        }

        if self.const_count == self.const_capacity {
            let new_cap = if self.const_capacity == 0 {
                8
            } else {
                self.const_capacity.saturating_mul(2).max(16)
            };
            let old_bytes = usize::from(self.const_capacity) * size_of::<Value>();
            let new_bytes = usize::from(new_cap) * size_of::<Value>();
            // SAFETY: Reallocating `self.constants` owned by `alloc`.
            let raw =
                unsafe { alloc.realloc_bytes(self.constants.cast::<u8>(), old_bytes, new_bytes) }
                    .ok_or("Out of memory")?;
            self.constants = raw.as_ptr().cast::<Value>();
            self.const_capacity = new_cap;
        }

        let idx = self.const_count;
        // SAFETY: `idx < self.const_capacity`.
        unsafe {
            *self.constants.add(idx as usize) = val;
        }
        self.const_count += 1;

        if can_map {
            dedup_map.set(alloc, val, Value::from_i48(i64::from(idx)))?;
        }
        Ok(idx)
    }

    /// Appends a child prototype pointer to `self.protos` and returns its `u16` index.
    pub fn add_child_proto(
        &mut self,
        alloc: &mut GcAllocator,
        child: *mut Self,
    ) -> Result<u16, &'static str> {
        if self.proto_count == u16::MAX {
            return Err("Too many nested functions");
        }
        if self.proto_count == self.proto_capacity {
            let new_cap = if self.proto_capacity == 0 {
                4
            } else {
                self.proto_capacity.saturating_mul(2)
            };
            let old_bytes = usize::from(self.proto_capacity) * size_of::<*mut Self>();
            let new_bytes = usize::from(new_cap) * size_of::<*mut Self>();
            // SAFETY: Reallocating `self.protos` owned by `alloc`.
            let raw =
                unsafe { alloc.realloc_bytes(self.protos.cast::<u8>(), old_bytes, new_bytes) }
                    .ok_or("Out of memory")?;
            self.protos = raw.as_ptr().cast::<*mut Self>();
            self.proto_capacity = new_cap;
        }
        let idx = self.proto_count;
        // SAFETY: `idx < self.proto_capacity`.
        unsafe { *self.protos.add(idx as usize) = child };
        self.proto_count += 1;
        Ok(idx)
    }

    /// Appends or reuses an `UpvalDesc` in `self.upvalues` and returns its `u8` index.
    pub fn add_upvalue(
        &mut self,
        alloc: &mut GcAllocator,
        desc: UpvalDesc,
    ) -> Result<u8, &'static str> {
        for i in 0..self.upvalue_count {
            // SAFETY: `i < self.upvalue_count`.
            let existing = unsafe { *self.upvalues.add(i as usize) };
            if existing.index == desc.index && existing.is_local == desc.is_local {
                return Ok(i);
            }
        }

        if usize::from(self.upvalue_count) >= ELARA_MAX_UPVALUES {
            return Err("Too many upvalues (max 255)");
        }

        let old_count = usize::from(self.upvalue_count);
        let new_count = old_count + 1;
        let old_bytes = old_count * size_of::<UpvalDesc>();
        let new_bytes = new_count * size_of::<UpvalDesc>();
        // SAFETY: Reallocating `self.upvalues` owned by `alloc`.
        let raw = unsafe { alloc.realloc_bytes(self.upvalues.cast::<u8>(), old_bytes, new_bytes) }
            .ok_or("Out of memory")?;
        self.upvalues = raw.as_ptr().cast::<UpvalDesc>();

        let idx = self.upvalue_count;
        // SAFETY: `idx < new_count`.
        unsafe { *self.upvalues.add(idx as usize) = desc };
        self.upvalue_count += 1;
        Ok(idx)
    }

    /// Resolves the 1-based source line number for instruction index `pc` from the RLE table.
    #[must_use]
    pub fn get_line(&self, pc: u32) -> u32 {
        let mut accum = 0u32;
        for i in 0..self.line_run_count {
            // SAFETY: `i < self.line_run_count`.
            let run = unsafe { *self.line_runs.add(i as usize) };
            accum = accum.saturating_add(run.count);
            if pc < accum {
                return run.line;
            }
        }
        0
    }

    /// Frees all auxiliary arrays (`code`, `ic_slots`, `constants`, `protos`, `upvalues`, `line_runs`)
    /// owned by this `ObjProto`.
    pub fn free_proto(&mut self, alloc: &mut GcAllocator) {
        if !self.code.is_null() && self.code_capacity > 0 {
            let code_bytes = (self.code_capacity as usize) * size_of::<Instruction>();
            let ic_bytes = (self.code_capacity as usize) * size_of::<u16>();
            // SAFETY: Both buffers were allocated with `code_capacity` elements by `alloc`.
            unsafe {
                alloc.free_bytes(self.code.cast::<u8>(), code_bytes);
                alloc.free_bytes(self.ic_slots.cast::<u8>(), ic_bytes);
            }
            self.code = core::ptr::null_mut();
            self.ic_slots = core::ptr::null_mut();
            self.code_count = 0;
            self.code_capacity = 0;
        }
        if !self.constants.is_null() && self.const_capacity > 0 {
            let bytes = usize::from(self.const_capacity) * size_of::<Value>();
            // SAFETY: `constants` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.constants.cast::<u8>(), bytes) };
            self.constants = core::ptr::null_mut();
            self.const_count = 0;
            self.const_capacity = 0;
        }
        if !self.protos.is_null() && self.proto_capacity > 0 {
            let bytes = usize::from(self.proto_capacity) * size_of::<*mut Self>();
            // SAFETY: `protos` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.protos.cast::<u8>(), bytes) };
            self.protos = core::ptr::null_mut();
            self.proto_count = 0;
            self.proto_capacity = 0;
        }
        if !self.upvalues.is_null() && self.upvalue_count > 0 {
            let bytes = usize::from(self.upvalue_count) * size_of::<UpvalDesc>();
            // SAFETY: `upvalues` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.upvalues.cast::<u8>(), bytes) };
            self.upvalues = core::ptr::null_mut();
            self.upvalue_count = 0;
        }
        if !self.line_runs.is_null() && self.line_run_capacity > 0 {
            let bytes = (self.line_run_capacity as usize) * size_of::<LineRun>();
            // SAFETY: `line_runs` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.line_runs.cast::<u8>(), bytes) };
            self.line_runs = core::ptr::null_mut();
            self.line_run_count = 0;
            self.line_run_capacity = 0;
        }
    }

    fn grow_code_and_ic(&mut self, alloc: &mut GcAllocator, new_cap: u32) -> Option<()> {
        let old_code_bytes = (self.code_capacity as usize) * size_of::<Instruction>();
        let new_code_bytes = (new_cap as usize).checked_mul(size_of::<Instruction>())?;
        let old_ic_bytes = (self.code_capacity as usize) * size_of::<u16>();
        let new_ic_bytes = (new_cap as usize).checked_mul(size_of::<u16>())?;

        // SAFETY: Reallocating `code` and `ic_slots` owned by `alloc`.
        let new_code =
            unsafe { alloc.realloc_bytes(self.code.cast::<u8>(), old_code_bytes, new_code_bytes) }?;
        self.code = new_code.as_ptr().cast::<Instruction>();

        let new_ic = (unsafe {
            alloc.realloc_bytes(self.ic_slots.cast::<u8>(), old_ic_bytes, new_ic_bytes)
        })?;
        self.ic_slots = new_ic.as_ptr().cast::<u16>();
        self.code_capacity = new_cap;
        Some(())
    }

    fn append_line_rle(&mut self, alloc: &mut GcAllocator, line: u32) -> Option<()> {
        if self.line_run_count > 0 {
            // SAFETY: `self.line_run_count - 1 < self.line_run_capacity`.
            let last = unsafe { &mut *self.line_runs.add((self.line_run_count - 1) as usize) };
            if last.line == line {
                last.count += 1;
                return Some(());
            }
        }

        if self.line_run_count == self.line_run_capacity {
            let new_cap = if self.line_run_capacity == 0 {
                8
            } else {
                self.line_run_capacity.checked_mul(2)?
            };
            let old_bytes = (self.line_run_capacity as usize) * size_of::<LineRun>();
            let new_bytes = (new_cap as usize).checked_mul(size_of::<LineRun>())?;
            // SAFETY: Reallocating `line_runs` owned by `alloc`.
            let raw =
                unsafe { alloc.realloc_bytes(self.line_runs.cast::<u8>(), old_bytes, new_bytes) }?;
            self.line_runs = raw.as_ptr().cast::<LineRun>();
            self.line_run_capacity = new_cap;
        }

        // SAFETY: `self.line_run_count < self.line_run_capacity`.
        unsafe {
            *self.line_runs.add(self.line_run_count as usize) = LineRun { line, count: 1 };
        }
        self.line_run_count += 1;
        Some(())
    }
}

/// Checks whether two constant pool values `a` and `b` have both the same value AND the same
/// numeric/type representation (so integer `1` and float `1.0`, or `-0.0` and `+0.0`, stay distinct
/// in `K[i]` while two identical `i48`, `ObjBoxedI64`, `f64`, or `ObjString` values deduplicate).
#[inline]
fn same_constant_rep(a: Value, b: Value) -> bool {
    if a.is_float() != b.is_float() {
        return false;
    }
    if a.is_float() {
        return a.as_f64().to_bits() == b.as_f64().to_bits();
    }
    a.val_equal(b)
}
