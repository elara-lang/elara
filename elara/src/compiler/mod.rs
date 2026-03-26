//! Single-pass Pratt compiler, lexical scope resolver, function prototypes, and 32-bit ISA.

pub mod disasm;
pub mod expr;
mod fold;
mod jump;
pub mod opcode;
pub mod proto;
pub mod scope;
mod stmt;

pub use disasm::{disassemble_instruction, disassemble_proto};
pub use expr::{BinOp, UnOp, encode_rk, encode_sc};
pub use opcode::{
    ELARA_MAX_BX, ELARA_OPCODE_COUNT, ELARA_SBX_BIAS, ELARA_SBX_MAX, ELARA_SBX_MIN, ELARA_SJ_BIAS,
    ELARA_SJ_MAX, ELARA_SJ_MIN, InstrFormat, Instruction, OpCode, decode_rk, encode_rk_const,
};
pub use proto::{
    ELARA_MAX_CONSTANTS, ELARA_MAX_LOCALS, ELARA_MAX_REGISTERS, ELARA_MAX_UPVALUES, LineRun,
    NO_JUMP, ObjProto, UpvalDesc,
};
pub use scope::{
    CompileError, ExprDesc, ExprKind, FunctionScope, INTRINSIC_NAMES, Local, LoopScope,
    resolve_assignable, resolve_variable,
};
pub use stmt::{RangeForInfo, TableBuilder, ceil_log2, newtable_hash_log2};
