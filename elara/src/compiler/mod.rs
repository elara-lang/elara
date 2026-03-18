//! Single-pass Pratt compiler, lexical scope resolver, function prototypes, and 32-bit ISA.

pub mod expr;
mod fold;
mod jump;
pub mod opcode;
pub mod proto;
pub mod scope;

pub use expr::{BinOp, UnOp, encode_rk, encode_sc};
pub use opcode::{ELARA_MAX_BX, ELARA_SBX_BIAS, ELARA_SJ_BIAS, Instruction, OpCode};
pub use proto::{
    ELARA_MAX_CONSTANTS, ELARA_MAX_LOCALS, ELARA_MAX_REGISTERS, ELARA_MAX_UPVALUES, LineRun,
    NO_JUMP, ObjProto, UpvalDesc,
};
pub use scope::{
    CompileError, ExprDesc, ExprKind, FunctionScope, INTRINSIC_NAMES, Local, LoopScope,
    resolve_assignable, resolve_variable,
};
