//! Zero-copy UTF-8 lexical scanner for the Elara language.

mod keyword;
mod number;
mod string;
pub mod token;

pub use keyword::classify_keyword;
pub use string::unescape_string;
pub use token::{ELARA_MAX_IDENT_LEN, ELARA_MAX_INTERP_DEPTH, Token, TokenKind};

pub struct Lexer<'a> {
    pub(super) source: &'a str,
    pub(super) bytes: &'a [u8],
    pub(super) start: usize,
    pub(super) current: usize,
    pub(super) line: u32,
    pub(super) column: u16,
    pub(super) token_line: u32,
    pub(super) token_column: u16,
    pub(super) interp_depth: usize,
    pub(super) brace_counters: [u8; ELARA_MAX_INTERP_DEPTH],
    pub(super) interp_is_triple: [bool; ELARA_MAX_INTERP_DEPTH],
}

impl<'a> Lexer<'a> {
    /// Creates a new zero-copy lexer over `source`, skipping any initial UTF-8 BOM
    /// and an optional Unix shebang line (`#!...`) at the start of the file.
    #[must_use]
    pub fn new(source: &'a str) -> Self {
        let bytes = source.as_bytes();
        let mut current = 0;
        let mut line = 1u32;
        let mut column = 1u16;

        // Skip UTF-8 BOM (0xEF 0xBB 0xBF) if present at offset 0.
        if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            current = 3;
        }

        // Skip optional Unix shebang (`#!` up to `\n` or EOF) immediately after optional BOM.
        if bytes.len() >= current + 2 && bytes[current] == b'#' && bytes[current + 1] == b'!' {
            current += 2;
            while current < bytes.len() && bytes[current] != b'\n' {
                current += 1;
            }
            if current < bytes.len() && bytes[current] == b'\n' {
                current += 1;
                line += 1;
                column = 1;
            }
        }

        Self {
            source,
            bytes,
            start: current,
            current,
            line,
            column,
            token_line: line,
            token_column: column,
            interp_depth: 0,
            brace_counters: [0; ELARA_MAX_INTERP_DEPTH],
            interp_is_triple: [false; ELARA_MAX_INTERP_DEPTH],
        }
    }

    /// Scans and returns the next lexical token from the source buffer.
    pub fn next_token(&mut self) -> Token<'a> {
        if let Some(err_tok) = self.skip_whitespace_and_comments() {
            return err_tok;
        }

        self.start = self.current;
        self.token_line = self.line;
        self.token_column = self.column;

        if self.is_at_end() {
            return self.make_token(TokenKind::Eof);
        }

        let b = self.advance();

        if b.is_ascii_alphabetic() || b == b'_' {
            return self.scan_identifier();
        }

        if b.is_ascii_digit() {
            return self.scan_number(b);
        }

        match b {
            b'(' => self.make_token(TokenKind::LParen),
            b')' => self.make_token(TokenKind::RParen),
            b'{' => {
                if self.interp_depth > 0 {
                    let idx = self.interp_depth - 1;
                    self.brace_counters[idx] = self.brace_counters[idx].saturating_add(1);
                }
                self.make_token(TokenKind::LBrace)
            }
            b'}' => {
                if self.interp_depth > 0 {
                    let idx = self.interp_depth - 1;
                    if self.brace_counters[idx] > 0 {
                        self.brace_counters[idx] -= 1;
                        return self.make_token(TokenKind::RBrace);
                    }
                    let is_triple = self.interp_is_triple[idx];
                    return self.scan_double_quoted_segment(false, is_triple);
                }
                self.make_token(TokenKind::RBrace)
            }
            b'"' => {
                let is_triple = self.peek() == b'"' && self.peek_next() == b'"';
                if is_triple {
                    self.advance();
                    self.advance();
                }
                self.scan_double_quoted_segment(true, is_triple)
            }
            b'\'' => self.scan_single_quoted_string(),
            b'[' => self.make_token(TokenKind::LBracket),
            b']' => self.make_token(TokenKind::RBracket),
            b',' => self.make_token(TokenKind::Comma),
            b':' => self.make_token(TokenKind::Colon),
            b';' => self.make_token(TokenKind::Semicolon),
            b'#' => self.make_token(TokenKind::Hash),
            b'.' => {
                if self.match_byte(b'.') {
                    if self.match_byte(b'.') {
                        self.make_token(TokenKind::Ellipsis)
                    } else if self.match_byte(b'=') {
                        self.make_token(TokenKind::DotDotEq)
                    } else {
                        self.make_token(TokenKind::DotDot)
                    }
                } else {
                    self.make_token(TokenKind::Dot)
                }
            }
            b'?' => {
                if self.match_byte(b'?') {
                    self.make_token(TokenKind::QuestionQuestion)
                } else {
                    self.make_token(TokenKind::Question)
                }
            }
            b'+' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::PlusAssign)
                } else {
                    self.make_token(TokenKind::Plus)
                }
            }
            b'-' => {
                if self.match_byte(b'>') {
                    self.make_token(TokenKind::Arrow)
                } else if self.match_byte(b'=') {
                    self.make_token(TokenKind::MinusAssign)
                } else {
                    self.make_token(TokenKind::Minus)
                }
            }
            b'*' => {
                if self.match_byte(b'*') {
                    self.make_token(TokenKind::StarStar)
                } else if self.match_byte(b'=') {
                    self.make_token(TokenKind::StarAssign)
                } else {
                    self.make_token(TokenKind::Star)
                }
            }
            b'/' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::SlashAssign)
                } else {
                    self.make_token(TokenKind::Slash)
                }
            }
            b'%' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::PercentAssign)
                } else {
                    self.make_token(TokenKind::Percent)
                }
            }
            b'=' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::Eq)
                } else {
                    self.make_token(TokenKind::Assign)
                }
            }
            b'!' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::Ne)
                } else {
                    self.error_token("Unexpected character '!'")
                }
            }
            b'~' => {
                if self.match_byte(b'=') {
                    self.error_token("Invalid operator '~='; use '!=' for inequality")
                } else {
                    self.make_token(TokenKind::Tilde)
                }
            }
            b'<' => {
                if self.match_byte(b'<') {
                    if self.match_byte(b'=') {
                        self.make_token(TokenKind::ShlAssign)
                    } else {
                        self.make_token(TokenKind::Shl)
                    }
                } else if self.match_byte(b'=') {
                    self.make_token(TokenKind::Le)
                } else {
                    self.make_token(TokenKind::Lt)
                }
            }
            b'>' => {
                if self.match_byte(b'>') {
                    if self.match_byte(b'=') {
                        self.make_token(TokenKind::ShrAssign)
                    } else {
                        self.make_token(TokenKind::Shr)
                    }
                } else if self.match_byte(b'=') {
                    self.make_token(TokenKind::Ge)
                } else {
                    self.make_token(TokenKind::Gt)
                }
            }
            b'&' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::AmpAssign)
                } else {
                    self.make_token(TokenKind::Amp)
                }
            }
            b'|' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::PipeAssign)
                } else {
                    self.make_token(TokenKind::Pipe)
                }
            }
            b'^' => {
                if self.match_byte(b'=') {
                    self.make_token(TokenKind::CaretAssign)
                } else {
                    self.make_token(TokenKind::Caret)
                }
            }
            _ => self.error_token("Unexpected character"),
        }
    }

    #[inline]
    pub(super) fn is_at_end(&self) -> bool {
        self.current >= self.bytes.len()
    }

    #[inline]
    pub(super) fn peek(&self) -> u8 {
        if self.is_at_end() {
            0
        } else {
            self.bytes[self.current]
        }
    }

    #[inline]
    pub(super) fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.bytes.len() {
            0
        } else {
            self.bytes[self.current + 1]
        }
    }

    #[inline]
    pub(super) fn peek_at(&self, offset: usize) -> u8 {
        let idx = self.current + offset;
        if idx >= self.bytes.len() {
            0
        } else {
            self.bytes[idx]
        }
    }

    #[inline]
    pub(super) fn advance(&mut self) -> u8 {
        let b = self.bytes[self.current];
        self.current += 1;
        if b == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column = self.column.saturating_add(1);
        }
        b
    }

    #[inline]
    pub(super) fn match_byte(&mut self, expected: u8) -> bool {
        if self.is_at_end() || self.bytes[self.current] != expected {
            return false;
        }
        self.advance();
        true
    }

    #[inline]
    pub(super) fn make_token(&self, kind: TokenKind) -> Token<'a> {
        Token {
            kind,
            lexeme: &self.source[self.start..self.current],
            line: self.token_line,
            column: self.token_column,
        }
    }

    #[inline]
    pub(super) fn error_token(&self, message: &'static str) -> Token<'a> {
        Token {
            kind: TokenKind::Error,
            lexeme: message,
            line: self.token_line,
            column: self.token_column,
        }
    }

    fn skip_whitespace_and_comments(&mut self) -> Option<Token<'a>> {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                b'/' => {
                    if self.peek_next() == b'/' {
                        self.advance();
                        self.advance();
                        while !self.is_at_end() && self.peek() != b'\n' {
                            self.advance();
                        }
                    } else if self.peek_next() == b'*' {
                        self.start = self.current;
                        self.token_line = self.line;
                        self.token_column = self.column;
                        self.advance();
                        self.advance();
                        let mut depth = 1u32;
                        while depth > 0 {
                            if self.is_at_end() {
                                return Some(self.error_token("Unterminated block comment"));
                            }
                            if self.peek() == b'/' && self.peek_next() == b'*' {
                                self.advance();
                                self.advance();
                                depth += 1;
                            } else if self.peek() == b'*' && self.peek_next() == b'/' {
                                self.advance();
                                self.advance();
                                depth -= 1;
                            } else {
                                self.advance();
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        None
    }
}
