use elara::lexer::{Lexer, Token, TokenKind};

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

