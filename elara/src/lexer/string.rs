//! String literal scanning, nested interpolation state machine, and zero-copy unescaping.

use super::{ELARA_MAX_INTERP_DEPTH, Lexer, Token, TokenKind};

impl<'a> Lexer<'a> {
    pub(super) fn scan_single_quoted_string(&mut self) -> Token<'a> {
        while !self.is_at_end() {
            let b = self.advance();
            match b {
                b'\'' => return self.make_token(TokenKind::String),
                b'\\' => {
                    if self.is_at_end() {
                        return self.error_token("Unterminated string literal");
                    }
                    let esc = self.advance();
                    if esc != b'\'' && esc != b'\\' {
                        return self.error_token("Invalid string escape sequence");
                    }
                }
                _ => {}
            }
        }
        self.error_token("Unterminated string literal")
    }

    pub(super) fn scan_double_quoted_segment(
        &mut self,
        is_start: bool,
        is_triple: bool,
    ) -> Token<'a> {
        while !self.is_at_end() {
            if is_triple {
                if self.peek() == b'"' && self.peek_next() == b'"' && self.peek_at(2) == b'"' {
                    self.advance();
                    self.advance();
                    self.advance();
                    return if is_start {
                        self.make_token(TokenKind::String)
                    } else {
                        self.interp_depth -= 1;
                        self.make_token(TokenKind::InterpEnd)
                    };
                }
            } else if self.peek() == b'"' {
                self.advance();
                return if is_start {
                    self.make_token(TokenKind::String)
                } else {
                    self.interp_depth -= 1;
                    self.make_token(TokenKind::InterpEnd)
                };
            } else if self.peek() == b'\n' {
                return self.error_token("Unterminated string literal");
            }

            let b = self.advance();
            match b {
                b'{' => {
                    if is_start {
                        if self.interp_depth >= ELARA_MAX_INTERP_DEPTH {
                            return self
                                .error_token("Maximum string interpolation depth (16) exceeded");
                        }
                        self.brace_counters[self.interp_depth] = 0;
                        self.interp_is_triple[self.interp_depth] = is_triple;
                        self.interp_depth += 1;
                        return self.make_token(TokenKind::InterpStart);
                    }
                    self.brace_counters[self.interp_depth - 1] = 0;
                    return self.make_token(TokenKind::InterpMid);
                }
                b'\\' => {
                    if let Err(msg) = self.validate_escape_in_place() {
                        return self.error_token(msg);
                    }
                }
                _ => {}
            }
        }
        self.error_token("Unterminated string literal")
    }

    fn validate_escape_in_place(&mut self) -> Result<(), &'static str> {
        if self.is_at_end() {
            return Err("Unterminated string literal");
        }
        let esc = self.advance();
        match esc {
            b'"' | b'\'' | b'\\' | b'{' | b'}' | b'n' | b'r' | b't' | b'0' => Ok(()),
            b'x' => {
                for _ in 0..2 {
                    if self.is_at_end() {
                        return Err("Unterminated string literal");
                    }
                    let d = self.advance();
                    if !d.is_ascii_hexdigit() {
                        return Err("Invalid string escape sequence");
                    }
                }
                Ok(())
            }
            b'u' => {
                if self.is_at_end() {
                    return Err("Unterminated string literal");
                }
                if self.advance() != b'{' {
                    return Err("Invalid string escape sequence");
                }
                let mut count = 0usize;
                let mut codepoint = 0u32;
                loop {
                    if self.is_at_end() {
                        return Err("Unterminated string literal");
                    }
                    let d = self.advance();
                    if d == b'}' {
                        break;
                    }
                    let Some(val) = hex_val(d) else {
                        return Err("Invalid string escape sequence");
                    };
                    codepoint = (codepoint << 4) | u32::from(val);
                    count += 1;
                    if count > 6 {
                        return Err("Invalid string escape sequence");
                    }
                }
                if count == 0 || char::from_u32(codepoint).is_none() {
                    return Err("Invalid string escape sequence");
                }
                Ok(())
            }
            _ => Err("Invalid string escape sequence"),
        }
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
