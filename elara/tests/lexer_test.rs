use elara::lexer::{Lexer, Token, TokenKind, unescape_string};

fn collect_tokens(src: &str) -> Vec<Token<'_>> {
    let mut lexer = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        let tok = lexer.next_token();
        let is_eof = tok.kind == TokenKind::Eof || tok.kind == TokenKind::Error;
        out.push(tok);
        if is_eof {
            break;
        }
    }
    out
}

#[test]
fn lexer_skips_utf8_bom_unix_shebang_and_whitespace_tracking_lines_and_columns() {
    let src = "\u{FEFF}#!/usr/bin/env elara\n  foo + bar\n\tbaz";
    let tokens = collect_tokens(src);

    assert_eq!(tokens.len(), 5);
    assert_eq!(
        tokens[0],
        Token {
            kind: TokenKind::Ident,
            lexeme: "foo",
            line: 2,
            column: 3,
        }
    );
    assert_eq!(
        tokens[1],
        Token {
            kind: TokenKind::Plus,
            lexeme: "+",
            line: 2,
            column: 7,
        }
    );
    assert_eq!(
        tokens[2],
        Token {
            kind: TokenKind::Ident,
            lexeme: "bar",
            line: 2,
            column: 9,
        }
    );
    assert_eq!(
        tokens[3],
        Token {
            kind: TokenKind::Ident,
            lexeme: "baz",
            line: 3,
            column: 2,
        }
    );
    assert_eq!(tokens[4].kind, TokenKind::Eof);

    // Ensure `#` inside regular code is still lexed as TokenKind::Hash (length operator).
    let hash_tokens = collect_tokens("#items");
    assert_eq!(hash_tokens[0].kind, TokenKind::Hash);
    assert_eq!(hash_tokens[0].lexeme, "#");
    assert_eq!(hash_tokens[1].kind, TokenKind::Ident);
    assert_eq!(hash_tokens[1].lexeme, "items");
}

#[test]
fn lexer_handles_nested_block_comments_and_rejects_tilde_eq() {
    let src = "a // line comment\n/* outer /* nested /* deep */ still outer */ done */ b != c";
    let tokens = collect_tokens(src);
    assert_eq!(tokens.len(), 5);
    assert_eq!(tokens[0].lexeme, "a");
    assert_eq!(tokens[1].lexeme, "b");
    assert_eq!(tokens[2].kind, TokenKind::Ne);
    assert_eq!(tokens[2].lexeme, "!=");
    assert_eq!(tokens[3].lexeme, "c");
    assert_eq!(tokens[4].kind, TokenKind::Eof);

    // Unterminated nested block comment MUST fail with exact error message.
    let unclosed = collect_tokens("x /* outer /* inner */");
    assert_eq!(unclosed.len(), 2);
    assert_eq!(unclosed[1].kind, TokenKind::Error);
    assert_eq!(unclosed[1].lexeme, "Unterminated block comment");

    // Forbidden `~=` operator MUST fail with exact error message.
    let tilde_eq = collect_tokens("a ~= b");
    assert_eq!(tilde_eq.len(), 2);
    assert_eq!(tilde_eq[1].kind, TokenKind::Error);
    assert_eq!(
        tilde_eq[1].lexeme,
        "Invalid operator '~='; use '!=' for inequality"
    );
}

#[test]
fn lexer_scans_all_delimiters_and_compound_operators() {
    let src = "( ) { } [ ] , . .. ..= ... : ; ? -> = + - * / % ** += -= *= /= %= == != < <= > >= & | ^ ~ << >> &= |= ^= <<= >>= ?? #";
    let tokens = collect_tokens(src);
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::LBrace,
            TokenKind::RBrace,
            TokenKind::LBracket,
            TokenKind::RBracket,
            TokenKind::Comma,
            TokenKind::Dot,
            TokenKind::DotDot,
            TokenKind::DotDotEq,
            TokenKind::Ellipsis,
            TokenKind::Colon,
            TokenKind::Semicolon,
            TokenKind::Question,
            TokenKind::Arrow,
            TokenKind::Assign,
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::StarStar,
            TokenKind::PlusAssign,
            TokenKind::MinusAssign,
            TokenKind::StarAssign,
            TokenKind::SlashAssign,
            TokenKind::PercentAssign,
            TokenKind::Eq,
            TokenKind::Ne,
            TokenKind::Lt,
            TokenKind::Le,
            TokenKind::Gt,
            TokenKind::Ge,
            TokenKind::Amp,
            TokenKind::Pipe,
            TokenKind::Caret,
            TokenKind::Tilde,
            TokenKind::Shl,
            TokenKind::Shr,
            TokenKind::AmpAssign,
            TokenKind::PipeAssign,
            TokenKind::CaretAssign,
            TokenKind::ShlAssign,
            TokenKind::ShrAssign,
            TokenKind::QuestionQuestion,
            TokenKind::Hash,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexer_scans_all_24_keywords_in_o1_and_rejects_overlong_ident() {
    let src = "let const defer fn return if else while for in break continue true false nil and or not import export as yield self super custom_ident x";
    let tokens = collect_tokens(src);
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::KwLet,
            TokenKind::KwConst,
            TokenKind::KwDefer,
            TokenKind::KwFn,
            TokenKind::KwReturn,
            TokenKind::KwIf,
            TokenKind::KwElse,
            TokenKind::KwWhile,
            TokenKind::KwFor,
            TokenKind::KwIn,
            TokenKind::KwBreak,
            TokenKind::KwContinue,
            TokenKind::KwTrue,
            TokenKind::KwFalse,
            TokenKind::KwNil,
            TokenKind::KwAnd,
            TokenKind::KwOr,
            TokenKind::KwNot,
            TokenKind::KwImport,
            TokenKind::KwExport,
            TokenKind::KwAs,
            TokenKind::KwYield,
            TokenKind::KwSelf,
            TokenKind::KwSuper,
            TokenKind::Ident,
            TokenKind::Ident,
            TokenKind::Eof,
        ]
    );

    // 255-byte identifier is valid; 256-byte identifier MUST fail with exact error message.
    let valid_ident = "a".repeat(255);
    let valid_tokens = collect_tokens(&valid_ident);
    assert_eq!(valid_tokens[0].kind, TokenKind::Ident);
    assert_eq!(valid_tokens[0].lexeme.len(), 255);

    let overlong_ident = "a".repeat(256);
    let err_tokens = collect_tokens(&overlong_ident);
    assert_eq!(err_tokens[0].kind, TokenKind::Error);
    assert_eq!(
        err_tokens[0].lexeme,
        "Identifier exceeds maximum length of 255 bytes"
    );
}

#[test]
fn lexer_scans_integer_radices_with_underscores_floats_and_stepped_range_dots() {
    let src = "42 1_000_000 0xFF_00 0b1010_0101 0o755 3.14159 1e-9 2.5E+12 1..10:2 0..=5";
    let tokens = collect_tokens(src);

    let pairs: Vec<(TokenKind, &str)> = tokens.into_iter().map(|t| (t.kind, t.lexeme)).collect();
    assert_eq!(
        pairs,
        vec![
            (TokenKind::Int, "42"),
            (TokenKind::Int, "1_000_000"),
            (TokenKind::Int, "0xFF_00"),
            (TokenKind::Int, "0b1010_0101"),
            (TokenKind::Int, "0o755"),
            (TokenKind::Float, "3.14159"),
            (TokenKind::Float, "1e-9"),
            (TokenKind::Float, "2.5E+12"),
            (TokenKind::Int, "1"),
            (TokenKind::DotDot, ".."),
            (TokenKind::Int, "10"),
            (TokenKind::Colon, ":"),
            (TokenKind::Int, "2"),
            (TokenKind::Int, "0"),
            (TokenKind::DotDotEq, "..="),
            (TokenKind::Int, "5"),
            (TokenKind::Eof, ""),
        ]
    );
}

#[test]
fn unescape_string_returns_zero_copy_slice_when_no_backslash_is_present() {
    let raw = "hello world without escapes";
    let mut scratch = Vec::new();
    let res = unescape_string(raw, &mut scratch).expect("should succeed");

    // Verify zero-copy pointer identity and zero scratch allocation.
    assert_eq!(res.as_ptr(), raw.as_bytes().as_ptr());
    assert_eq!(scratch.capacity(), 0);

    // Verify all supported escape sequences (`\"`, `\\`, `\{`, `\}`, `\n`, `\r`, `\t`, `\0`, `\xHH`, `\u{...}`).
    let escaped = r#"a\"b\\c\{d\}e\nf\rg\th\0i\x41\u{03BB}\u{1F680}"#;
    let unescaped = unescape_string(escaped, &mut scratch).expect("valid escapes");
    assert_eq!(
        unescaped,
        "a\"b\\c{d}e\nf\rg\th\0iA\u{03BB}\u{1F680}".as_bytes()
    );

    // Invalid escape sequences MUST return exact error message.
    assert_eq!(
        unescape_string(r"\q", &mut scratch),
        Err("Invalid string escape sequence")
    );
    assert_eq!(
        unescape_string(r"\x4G", &mut scratch),
        Err("Invalid string escape sequence")
    );
    assert_eq!(
        unescape_string(r"\u{110000}", &mut scratch),
        Err("Invalid string escape sequence")
    );
}
