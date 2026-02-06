//! Zero-copy UTF-8 lexical scanner for the Elara language.

pub mod token;

pub use token::{ELARA_MAX_IDENT_LEN, ELARA_MAX_INTERP_DEPTH, Token, TokenKind};

pub struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    start: usize,
    current: usize,
    line: u32,
    column: u16,
    token_line: u32,
    token_column: u16,
    #[allow(dead_code)]
    interp_depth: usize,
    #[allow(dead_code)]
    brace_counters: [u8; ELARA_MAX_INTERP_DEPTH],
    #[allow(dead_code)]
    interp_is_triple: [bool; ELARA_MAX_INTERP_DEPTH],
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
            b'{' => self.make_token(TokenKind::LBrace),
            b'}' => self.make_token(TokenKind::RBrace),
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
    fn is_at_end(&self) -> bool {
        self.current >= self.bytes.len()
    }

    #[inline]
    fn peek(&self) -> u8 {
        if self.is_at_end() {
            0
        } else {
            self.bytes[self.current]
        }
    }

    #[inline]
    fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.bytes.len() {
            0
        } else {
            self.bytes[self.current + 1]
        }
    }

    #[inline]
    fn advance(&mut self) -> u8 {
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
    fn match_byte(&mut self, expected: u8) -> bool {
        if self.is_at_end() || self.bytes[self.current] != expected {
            return false;
        }
        self.advance();
        true
    }

    #[inline]
    fn make_token(&self, kind: TokenKind) -> Token<'a> {
        Token {
            kind,
            lexeme: &self.source[self.start..self.current],
            line: self.token_line,
            column: self.token_column,
        }
    }

    #[inline]
    fn error_token(&self, message: &'static str) -> Token<'a> {
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
                        // Line comment (`//` until `\n` or EOF)
                        self.advance();
                        self.advance();
                        while !self.is_at_end() && self.peek() != b'\n' {
                            self.advance();
                        }
                    } else if self.peek_next() == b'*' {
                        // Nested block comment (`/* ... */`)
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

    fn scan_identifier(&mut self) -> Token<'a> {
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

    fn scan_number(&mut self, first: u8) -> Token<'a> {
        if first == b'0' {
            match self.peek() {
                b'x' | b'X' => {
                    self.advance();
                    return self.scan_radix_digits(is_hex_digit);
                }
                b'b' | b'B' => {
                    self.advance();
                    return self.scan_radix_digits(is_bin_digit);
                }
                b'o' | b'O' => {
                    self.advance();
                    return self.scan_radix_digits(is_oct_digit);
                }
                _ => {}
            }
        }

        self.consume_digits_with_underscores(is_dec_digit);

        let mut is_float = false;

        // Fractional part `.digits`, careful not to consume `..` range operator or `.ident` method access.
        if self.peek() == b'.' && is_dec_digit(self.peek_next()) {
            is_float = true;
            self.advance(); // consume '.'
            self.consume_digits_with_underscores(is_dec_digit);
        }

        // Scientific exponent `e` or `E`
        if self.peek() == b'e' || self.peek() == b'E' {
            is_float = true;
            self.advance(); // consume 'e' / 'E'
            if self.peek() == b'+' || self.peek() == b'-' {
                self.advance();
            }
            if !is_dec_digit(self.peek()) {
                return self.error_token("Invalid float exponent");
            }
            self.consume_digits_with_underscores(is_dec_digit);
        }

        self.make_token(if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        })
    }

    fn scan_radix_digits(&mut self, pred: fn(u8) -> bool) -> Token<'a> {
        let digits_start = self.current;
        self.consume_digits_with_underscores(pred);
        if self.current == digits_start {
            return self.error_token("Invalid integer literal");
        }
        self.make_token(TokenKind::Int)
    }

    fn consume_digits_with_underscores(&mut self, pred: fn(u8) -> bool) {
        while !self.is_at_end() {
            let b = self.peek();
            if pred(b) || (b == b'_' && pred(self.peek_next())) {
                self.advance();
            } else {
                break;
            }
        }
    }
}

#[inline]
const fn is_dec_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
const fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

#[inline]
const fn is_bin_digit(b: u8) -> bool {
    b == b'0' || b == b'1'
}

#[inline]
const fn is_oct_digit(b: u8) -> bool {
    b >= b'0' && b <= b'7'
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

/// Zero-copy string unescaper fast-path.
///
/// If `raw` contains no backslash (`\`), returns `Ok(raw.as_bytes())` directly from the source
/// slice without allocating or copying into `scratch`.
///
/// # Errors
///
/// Returns `Err("Invalid string escape sequence")` if `raw` contains a malformed escape sequence.
pub fn unescape_string<'a>(
    raw: &'a str,
    scratch: &'a mut Vec<u8>,
) -> Result<&'a [u8], &'static str> {
    let bytes = raw.as_bytes();
    if !bytes.contains(&b'\\') {
        return Ok(bytes);
    }

    scratch.clear();
    scratch.reserve(bytes.len());

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            scratch.push(b);
            i += 1;
            continue;
        }

        i += 1;
        if i >= bytes.len() {
            return Err("Invalid string escape sequence");
        }

        match bytes[i] {
            b'"' => {
                scratch.push(b'"');
                i += 1;
            }
            b'\'' => {
                scratch.push(b'\'');
                i += 1;
            }
            b'\\' => {
                scratch.push(b'\\');
                i += 1;
            }
            b'{' => {
                scratch.push(b'{');
                i += 1;
            }
            b'}' => {
                scratch.push(b'}');
                i += 1;
            }
            b'n' => {
                scratch.push(b'\n');
                i += 1;
            }
            b'r' => {
                scratch.push(b'\r');
                i += 1;
            }
            b't' => {
                scratch.push(b'\t');
                i += 1;
            }
            b'0' => {
                scratch.push(0);
                i += 1;
            }
            b'x' => {
                i += 1;
                if i + 2 > bytes.len() {
                    return Err("Invalid string escape sequence");
                }
                let hi = hex_val(bytes[i]).ok_or("Invalid string escape sequence")?;
                let lo = hex_val(bytes[i + 1]).ok_or("Invalid string escape sequence")?;
                scratch.push((hi << 4) | lo);
                i += 2;
            }
            b'u' => {
                i += 1;
                if i >= bytes.len() || bytes[i] != b'{' {
                    return Err("Invalid string escape sequence");
                }
                i += 1;
                let hex_start = i;
                let mut codepoint = 0u32;
                while i < bytes.len() && bytes[i] != b'}' {
                    let digit = hex_val(bytes[i]).ok_or("Invalid string escape sequence")?;
                    codepoint = (codepoint << 4) | u32::from(digit);
                    i += 1;
                    if i - hex_start > 6 {
                        return Err("Invalid string escape sequence");
                    }
                }
                if i == hex_start || i >= bytes.len() || bytes[i] != b'}' {
                    return Err("Invalid string escape sequence");
                }
                i += 1; // consume '}'
                let ch = char::from_u32(codepoint).ok_or("Invalid string escape sequence")?;
                let mut utf8_buf = [0u8; 4];
                let encoded = ch.encode_utf8(&mut utf8_buf);
                scratch.extend_from_slice(encoded.as_bytes());
            }
            _ => return Err("Invalid string escape sequence"),
        }
    }

    Ok(scratch.as_slice())
}

#[inline]
const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
