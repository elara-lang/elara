//! 32-bit register bytecode instruction formats (`iABC`, `iABx`, `iAsBx`, `isJ`) and 57-opcode ISA.

pub const ELARA_MAX_BX: u16 = 65535;
pub const ELARA_SBX_BIAS: i32 = 32767;
pub const ELARA_SJ_BIAS: i32 = 8_388_607;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    Nop = 0x00,
    Move = 0x01,
    LoadI = 0x02,
    LoadK = 0x03,
    LoadNil = 0x04,
    LoadBool = 0x05,
    GetUpval = 0x06,
    SetUpval = 0x07,
    Close = 0x08,
    NewTable = 0x09,
    GetTab = 0x0A,
    SetTab = 0x0B,
    GetI = 0x0C,
    SetI = 0x0D,
    GetField = 0x0E,
    SetField = 0x0F,
    SetList = 0x10,
    SelfMethod = 0x11,
    Add = 0x12,
    AddI = 0x13,
    Sub = 0x14,
    Mul = 0x15,
    Div = 0x16,
    Mod = 0x17,
    Pow = 0x18,
    Unm = 0x19,
    BAnd = 0x1A,
    BOr = 0x1B,
    BXor = 0x1C,
    Shl = 0x1D,
    Shr = 0x1E,
    BNot = 0x1F,
    Not = 0x20,
    Len = 0x21,
    ConcatN = 0x22,
    Eq = 0x23,
    Lt = 0x24,
    Le = 0x25,
    In = 0x26,
    Test = 0x27,
    TestSet = 0x28,
    Coalesce = 0x29,
    Jmp = 0x2A,
    ForPrep = 0x2B,
    ForLoop = 0x2C,
    ForIn = 0x2D,
    Range = 0x2E,
    Closure = 0x2F,
    Call = 0x30,
    TailCall = 0x31,
    Return = 0x32,
    Vararg = 0x33,
    Yield = 0x34,
    Import = 0x35,
    EqI = 0x36,
    LtI = 0x37,
    LeI = 0x38,
}

impl OpCode {
    /// Converts a raw byte into `Some(OpCode)` if within `0x00..=0x38`.
    #[inline(always)]
    #[must_use]
    pub const fn from_u8(raw: u8) -> Option<Self> {
        if raw <= Self::LeI as u8 {
            // SAFETY: `raw` is verified in `0x00..=0x38` matching contiguous `#[repr(u8)]` variants.
            Some(unsafe { core::mem::transmute::<u8, Self>(raw) })
        } else {
            None
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction(pub u32);

impl Instruction {
    #[inline(always)]
    #[must_use]
    pub const fn make_abc(op: OpCode, a: u8, b: u8, c: u8) -> Self {
        Self((op as u32) | ((a as u32) << 8) | ((c as u32) << 16) | ((b as u32) << 24))
    }

    #[inline(always)]
    #[must_use]
    pub const fn make_abx(op: OpCode, a: u8, bx: u16) -> Self {
        Self((op as u32) | ((a as u32) << 8) | ((bx as u32) << 16))
    }

    #[inline(always)]
    #[must_use]
    pub const fn make_asbx(op: OpCode, a: u8, sbx: i32) -> Self {
        Self::make_abx(op, a, (sbx + ELARA_SBX_BIAS) as u16)
    }

    #[inline(always)]
    #[must_use]
    pub const fn make_sj(op: OpCode, sj: i32) -> Self {
        let biased = ((sj + ELARA_SJ_BIAS) as u32) & 0x00FF_FFFF;
        Self((op as u32) | (biased << 8))
    }

    #[inline(always)]
    #[must_use]
    pub const fn opcode_raw(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    #[inline(always)]
    #[must_use]
    pub const fn opcode(self) -> Option<OpCode> {
        OpCode::from_u8(self.opcode_raw())
    }

    #[inline(always)]
    #[must_use]
    pub const fn a(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    #[inline(always)]
    #[must_use]
    pub const fn c(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    #[inline(always)]
    #[must_use]
    pub const fn sc(self) -> i8 {
        self.c() as i8
    }

    #[inline(always)]
    #[must_use]
    pub const fn b(self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    #[inline(always)]
    #[must_use]
    pub const fn bx(self) -> u16 {
        ((self.0 >> 16) & 0xFFFF) as u16
    }

    #[inline(always)]
    #[must_use]
    pub const fn sbx(self) -> i32 {
        (self.bx() as i32) - ELARA_SBX_BIAS
    }

    #[inline(always)]
    #[must_use]
    pub const fn sj(self) -> i32 {
        ((self.0 >> 8) as i32) - ELARA_SJ_BIAS
    }

    #[inline(always)]
    pub fn patch_a(&mut self, a: u8) {
        self.0 = (self.0 & !0x0000_FF00) | ((a as u32) << 8);
    }

    #[inline(always)]
    pub fn patch_b_c(&mut self, b: u8, c: u8) {
        self.0 = (self.0 & 0x0000_FFFF) | ((c as u32) << 16) | ((b as u32) << 24);
    }

    #[inline(always)]
    pub fn patch_sj(&mut self, sj: i32) {
        let biased = ((sj + ELARA_SJ_BIAS) as u32) & 0x00FF_FFFF;
        self.0 = (self.0 & 0x0000_00FF) | (biased << 8);
    }
}

