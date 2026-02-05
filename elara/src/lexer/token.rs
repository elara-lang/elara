//! Token taxonomy, Pratt binding powers, and zero-copy token representations.

pub const ELARA_MAX_IDENT_LEN: usize = 255;
pub const ELARA_MAX_INTERP_DEPTH: usize = 16;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Eof = 0,
    Error,

    // Literals & Identifiers
    Ident,       // foo, _bar, x1
    Int,         // 42, 0xFF, 0b1010, 0o77
    Float,       // 3.14, 1e-9
    String,      // "plain", """multiline""", or 'raw'
    InterpStart, // "prefix {
    InterpMid,   // } middle {
    InterpEnd,   // } suffix"

    // Reserved Keywords (24)
    KwLet,      // let
    KwConst,    // const
    KwDefer,    // defer
    KwFn,       // fn
    KwReturn,   // return
    KwIf,       // if
    KwElse,     // else
    KwWhile,    // while
    KwFor,      // for
    KwIn,       // in
    KwBreak,    // break
    KwContinue, // continue
    KwTrue,     // true
    KwFalse,    // false
    KwNil,      // nil
    KwAnd,      // and
    KwOr,       // or
    KwNot,      // not
    KwImport,   // import
    KwExport,   // export
    KwAs,       // as
    KwYield,    // yield
    KwSelf,     // self
    KwSuper,    // super

    // Delimiters & Punctuation
    LParen,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    Comma,     // ,
    Dot,       // .
    DotDot,    // ..
    DotDotEq,  // ..=
    Ellipsis,  // ...
    Colon,     // :
    Semicolon, // ;
    Question,  // ?
    Arrow,     // ->

    // Arithmetic & Assignment Operators
    Assign,        // =
    Plus,          // +
    Minus,         // -
    Star,          // *
    Slash,         // /
    Percent,       // %
    StarStar,      // **
    PlusAssign,    // +=
    MinusAssign,   // -=
    StarAssign,    // *=
    SlashAssign,   // /=
    PercentAssign, // %=

    // Relational & Equality Operators
    Eq, // ==
    Ne, // !=  (~= is forbidden)
    Lt, // <
    Le, // <=
    Gt, // >
    Ge, // >=

    // Bitwise, Coalescing & Length Operators
    Amp,              // &
    Pipe,             // |
    Caret,            // ^
    Tilde,            // ~
    Shl,              // <<
    Shr,              // >>
    AmpAssign,        // &=
    PipeAssign,       // |=
    CaretAssign,      // ^=
    ShlAssign,        // <<=
    ShrAssign,        // >>=
    QuestionQuestion, // ??
    Hash,             // #
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub lexeme: &'a str,
    pub line: u32,
    pub column: u16,
}
