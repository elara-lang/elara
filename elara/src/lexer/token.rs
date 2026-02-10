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

impl TokenKind {
    /// Returns the Pratt parser `(left_binding_power, right_binding_power)` for infix/postfix
    /// operators, or `(0, 0)` for non-infix tokens.
    #[inline]
    #[must_use]
    pub const fn binding_power(self) -> (u8, u8) {
        match self {
            // Level 01: Assignment (Right-associative)
            Self::Assign
            | Self::PlusAssign
            | Self::MinusAssign
            | Self::StarAssign
            | Self::SlashAssign
            | Self::PercentAssign
            | Self::AmpAssign
            | Self::PipeAssign
            | Self::CaretAssign
            | Self::ShlAssign
            | Self::ShrAssign => (10, 9),

            // Level 02: Ternary `? :` (Right-associative)
            Self::Question => (20, 19),

            // Level 03: Nil Coalescing `??` (Right-associative)
            Self::QuestionQuestion => (30, 29),

            // Level 04: Logical OR `or` (Left-associative)
            Self::KwOr => (40, 41),

            // Level 05: Logical AND `and` (Left-associative)
            Self::KwAnd => (50, 51),

            // Level 06: Equality `==`, `!=` (Left-associative)
            Self::Eq | Self::Ne => (60, 61),

            // Level 07: Relational `<`, `<=`, `>`, `>=`, `in` (Left-associative)
            Self::Lt | Self::Le | Self::Gt | Self::Ge | Self::KwIn => (70, 71),

            // Level 08: Range `..`, `..=` (Non-associative)
            Self::DotDot | Self::DotDotEq => (80, 80),

            // Level 09: Bitwise OR `|` (Left-associative)
            Self::Pipe => (90, 91),

            // Level 10: Bitwise XOR `^` (Left-associative)
            Self::Caret => (100, 101),

            // Level 11: Bitwise AND `&` (Left-associative)
            Self::Amp => (110, 111),

            // Level 12: Bitwise Shift `<<`, `>>` (Left-associative)
            Self::Shl | Self::Shr => (120, 121),

            // Level 13: Additive `+`, `-` (Left-associative)
            Self::Plus | Self::Minus => (130, 131),

            // Level 14: Multiplicative `*`, `/`, `%` (Left-associative)
            Self::Star | Self::Slash | Self::Percent => (140, 141),

            // Level 15: Exponentiation `**` (Right-associative)
            Self::StarStar => (150, 149),

            // Level 17: Postfix / Member Access `()`, `[]`, `.`, `:` (Left-associative)
            Self::LParen | Self::LBracket | Self::Dot | Self::Colon => (170, 171),

            _ => (0, 0),
        }
    }

    /// Returns the Pratt right binding power for prefix unary operators (`not`, `-`, `~`, `#`, `yield`),
    /// or `0` if the token is not a prefix unary operator.
    #[inline]
    #[must_use]
    pub const fn prefix_binding_power(self) -> u8 {
        match self {
            Self::KwNot | Self::Minus | Self::Tilde | Self::Hash | Self::KwYield => 160,
            _ => 0,
        }
    }
}
