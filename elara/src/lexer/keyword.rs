//! Identifier scanning and O(1) keyword recognition for the Elara lexer.

use super::{ELARA_MAX_IDENT_LEN, Lexer, Token, TokenKind};

impl<'a> Lexer<'a> {
    pub(super) fn scan_identifier(&mut self) -> Token<'a> {
        while !self.is_at_end() {
            let b = self.peek();
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        let len = self.current - self.start;
        if len > ELARA_MAX_IDENT_LEN {
            return self.error_token("Identifier exceeds maximum length of 255 bytes");
        }

        let lexeme = &self.source[self.start..self.current];
        let kind = classify_keyword(lexeme);
        self.make_token(kind)
    }
}

/// Dispatches keyword lookup in O(1) via `(lexeme.len(), first_byte)`.
/// Any identifier with `len < 2 || len > 8` immediately returns `TokenKind::Ident`.
#[inline]
#[must_use]
pub fn classify_keyword(lexeme: &str) -> TokenKind {
    let bytes = lexeme.as_bytes();
    let len = bytes.len();
    if !(2..=8).contains(&len) {
        return TokenKind::Ident;
    }

    match (len, bytes[0]) {
        (2, b'a') if lexeme == "as" => TokenKind::KwAs,
        (2, b'f') if lexeme == "fn" => TokenKind::KwFn,
        (2, b'i') => match lexeme {
            "if" => TokenKind::KwIf,
            "in" => TokenKind::KwIn,
            _ => TokenKind::Ident,
        },
        (2, b'o') if lexeme == "or" => TokenKind::KwOr,
        (3, b'a') if lexeme == "and" => TokenKind::KwAnd,
        (3, b'f') if lexeme == "for" => TokenKind::KwFor,
        (3, b'l') if lexeme == "let" => TokenKind::KwLet,
        (3, b'n') => match lexeme {
            "nil" => TokenKind::KwNil,
            "not" => TokenKind::KwNot,
            _ => TokenKind::Ident,
        },
        (4, b'e') if lexeme == "else" => TokenKind::KwElse,
        (4, b's') if lexeme == "self" => TokenKind::KwSelf,
        (4, b't') if lexeme == "true" => TokenKind::KwTrue,
        (5, b'b') if lexeme == "break" => TokenKind::KwBreak,
        (5, b'c') if lexeme == "const" => TokenKind::KwConst,
        (5, b'd') if lexeme == "defer" => TokenKind::KwDefer,
        (5, b'f') if lexeme == "false" => TokenKind::KwFalse,
        (5, b's') if lexeme == "super" => TokenKind::KwSuper,
        (5, b'w') if lexeme == "while" => TokenKind::KwWhile,
        (5, b'y') if lexeme == "yield" => TokenKind::KwYield,
        (6, b'e') if lexeme == "export" => TokenKind::KwExport,
        (6, b'i') if lexeme == "import" => TokenKind::KwImport,
        (6, b'r') if lexeme == "return" => TokenKind::KwReturn,
        (8, b'c') if lexeme == "continue" => TokenKind::KwContinue,
        _ => TokenKind::Ident,
    }
}

