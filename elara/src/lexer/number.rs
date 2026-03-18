//! Numeric literal scanning (decimal, hex, binary, octal, and IEEE-754 floats).

use super::{Lexer, Token, TokenKind};

impl<'a> Lexer<'a> {
    pub(super) fn scan_number(&mut self, first: u8) -> Token<'a> {
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
