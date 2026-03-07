//! Single-pass Pratt compiler, lexical scope resolver, function prototypes, and 32-bit ISA.

pub mod opcode;
pub mod proto;

pub use opcode::{ELARA_MAX_BX, ELARA_SBX_BIAS, ELARA_SJ_BIAS, Instruction, OpCode};
pub use proto::{
    ELARA_MAX_CONSTANTS, ELARA_MAX_LOCALS, ELARA_MAX_REGISTERS, ELARA_MAX_UPVALUES, LineRun,
    NO_JUMP, ObjProto, UpvalDesc,
};

