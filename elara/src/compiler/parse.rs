//! Single-pass Pratt parser driving `FunctionScope` codegen.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::compiler::expr::{BinOp, UnOp};
use crate::compiler::opcode::{Instruction, OpCode};
use crate::compiler::proto::{NO_JUMP, ObjProto};
use crate::compiler::scope::{
    CompileError, ExprDesc, ExprKind, FunctionScope, LoopScope, resolve_variable,
};
use crate::compiler::{ensure_string_constant, module_basename};
use crate::lexer::{Lexer, Token, TokenKind, unescape_string};
use crate::value::{ObjTable, ObjType, Value};
use crate::vm::state::ElaraState;

pub struct Parser<'a> {
    state: *mut ElaraState,
    lexer: Lexer<'a>,
    current: Token<'a>,
    previous: Token<'a>,
    scopes: Vec<FunctionScope<'a>>,
    scratch: Vec<u8>,
}

impl<'a> Parser<'a> {
    pub fn new(state: &mut ElaraState, source: &'a str, root: FunctionScope<'a>) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token();
        Self {
            state: state as *mut ElaraState,
            lexer,
            current,
            previous: Token {
                kind: TokenKind::Eof,
                lexeme: "",
                line: 1,
                column: 1,
            },
            scopes: vec![root],
            scratch: Vec::new(),
        }
    }

    pub fn into_scope(mut self) -> FunctionScope<'a> {
        self.scopes.pop().expect("root scope")
    }

    fn st(&mut self) -> &mut ElaraState {
        // SAFETY: `state` is borrowed for the Parser lifetime from `new`.
        unsafe { &mut *self.state }
    }

    fn sa(&mut self) -> (&mut FunctionScope<'a>, &mut ElaraState) {
        let state = unsafe { &mut *self.state };
        let scope = self.scopes.last_mut().expect("scope");
        (scope, state)
    }

    pub fn parse_chunk(&mut self) -> Result<(), CompileError> {
        while !self.check(TokenKind::Eof) {
            self.declaration()?;
            self.scopes.last_mut().unwrap().reset_regs();
        }
        Ok(())
    }

    fn declaration(&mut self) -> Result<(), CompileError> {
        if self.match_tok(TokenKind::KwLet) {
            self.parse_let(false)
        } else if self.match_tok(TokenKind::KwConst) {
            self.parse_let(true)
        } else if self.match_tok(TokenKind::KwFn) {
            self.parse_fn_decl()
        } else if self.match_tok(TokenKind::KwImport) {
            self.parse_import()
        } else {
            self.statement()
        }
    }

    fn statement(&mut self) -> Result<(), CompileError> {
        if self.match_tok(TokenKind::KwReturn) {
            self.parse_return()
        } else if self.match_tok(TokenKind::KwIf) {
            self.parse_if()
        } else if self.match_tok(TokenKind::KwWhile) {
            self.parse_while()
        } else if self.match_tok(TokenKind::KwBreak) {
            let line = self.previous.line;
            let (scope, state) = self.sa();
            scope.compile_break(&mut state.alloc, line)?;
            let _ = self.match_tok(TokenKind::Semicolon);
            Ok(())
        } else if self.check(TokenKind::LBrace) {
            self.parse_block()
        } else {
            let e = self.parse_expression()?;
            let line = self.previous.line;
            let (scope, state) = self.sa();
            let _ = scope.expr_to_any_reg(&mut state.alloc, e, line)?;
            scope.reset_regs();
            let _ = self.match_tok(TokenKind::Semicolon);
            Ok(())
        }
    }

    fn parse_block(&mut self) -> Result<(), CompileError> {
        self.consume(TokenKind::LBrace, "Expected '{'")?;
        self.scopes.last_mut().unwrap().begin_scope();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            self.declaration()?;
            self.scopes.last_mut().unwrap().reset_regs();
        }
        self.consume(TokenKind::RBrace, "Expected '}'")?;
        let line = self.previous.line;
        let (scope, state) = self.sa();
        scope.end_scope(&mut state.alloc, line)?;
        Ok(())
    }

    fn parse_let(&mut self, is_const: bool) -> Result<(), CompileError> {
        self.consume(TokenKind::Ident, "Expected variable name")?;
        let name = self.previous.lexeme;
        let line = self.previous.line;
        self.consume(TokenKind::Assign, "Expected '=' after variable name")?;
        let init = self.parse_expression()?;
        let (scope, state) = self.sa();
        let reg = scope.declare_local(name, is_const, false, line)?;
        scope.discharge_to_reg(&mut state.alloc, init, reg, line)?;
        scope.mark_initialized();
        scope.reset_regs();
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn parse_import(&mut self) -> Result<(), CompileError> {
        self.consume(TokenKind::String, "Expected module string")?;
        let str_tok = self.previous;
        let line = str_tok.line;
        let raw = Self::string_token_inner_slice(str_tok.lexeme);
        let spec_bytes =
            unescape_string(raw, &mut self.scratch).map_err(|e| CompileError::new(e, line))?;
        let spec_owned = core::str::from_utf8(spec_bytes)
            .map_err(|_| CompileError::new("Invalid UTF-8 string", line))?
            .to_owned();
        let alias = if self.match_tok(TokenKind::KwAs) {
            self.consume(TokenKind::Ident, "Expected import alias")?;
            Some(self.previous.lexeme)
        } else {
            None
        };
        let (scope, state) = self.sa();
        let reg = if let Some(a) = alias {
            scope.declare_local(a, true, false, line)?
        } else {
            let base = module_basename(spec_owned.as_str());
            let name_ref = if let Some(idx) = raw.rfind(base) {
                &raw[idx..idx + base.len()]
            } else {
                raw
            };
            scope.declare_local(name_ref, true, false, line)?
        };
        scope.mark_initialized();
        let bx = ensure_string_constant(
            scope,
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            spec_owned.as_bytes(),
            line,
        )?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abx(OpCode::Import, reg, bx),
            line,
        )?;
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn parse_fn_decl(&mut self) -> Result<(), CompileError> {
        self.consume(TokenKind::Ident, "Expected function name")?;
        let name = self.previous.lexeme;
        let line = self.previous.line;
        {
            let (scope, _) = self.sa();
            let reg = scope.declare_local(name, true, false, line)?;
            scope.mark_initialized();
            // `reg` used after compiling body
            let _ = reg;
        }
        let dst = {
            let scope = self.scopes.last().unwrap();
            scope.locals.last().unwrap().reg
        };
        self.compile_function_body(name, dst)?;
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn compile_function_body(&mut self, name: &'a str, dst: u8) -> Result<(), CompileError> {
        let line = self.previous.line;
        let (child, mut child_scope) = {
            let state = self.st();
            let child_name = state
                .strings
                .new_string(&mut state.alloc, &mut state.all_objects, 0, name.as_bytes())
                .ok_or_else(|| CompileError::new("Out of memory", line))?;
            let child = ObjProto::new(
                &mut state.alloc,
                &mut state.all_objects,
                0,
                child_name.as_ptr(),
            )
            .ok_or_else(|| CompileError::new("Out of memory", line))?;
            let dedup = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8)
                .ok_or_else(|| CompileError::new("Out of memory", line))?;
            (child, FunctionScope::new(child.as_ptr(), dedup.as_ptr()))
        };

        self.consume(TokenKind::LParen, "Expected '('")?;
        let mut nparams = 0u8;
        if !self.check(TokenKind::RParen) {
            loop {
                self.consume(TokenKind::Ident, "Expected parameter name")?;
                let pname = self.previous.lexeme;
                let pline = self.previous.line;
                child_scope.declare_local(pname, false, false, pline)?;
                child_scope.mark_initialized();
                nparams = nparams
                    .checked_add(1)
                    .ok_or_else(|| CompileError::new("Too many parameters", pline))?;
                if !self.match_tok(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume(TokenKind::RParen, "Expected ')'")?;
        unsafe {
            (*child.as_ptr()).num_params = nparams;
        }
        self.scopes.push(child_scope);

        self.parse_block()?;
        let mut finished = self.scopes.pop().expect("fn scope");
        {
            let state = self.st();
            finished.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                line,
            )?;
        }
        {
            let (scope, state) = self.sa();
            let bx = unsafe {
                (*scope.proto)
                    .add_child_proto(&mut state.alloc, child.as_ptr())
                    .map_err(|e| CompileError::new(e, line))?
            };
            scope.emit(
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, dst, bx),
                line,
            )?;
        }
        Ok(())
    }

    fn parse_return(&mut self) -> Result<(), CompileError> {
        let line = self.previous.line;
        if self.check(TokenKind::Semicolon)
            || self.check(TokenKind::RBrace)
            || self.check(TokenKind::Eof)
        {
            let (scope, state) = self.sa();
            scope.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                line,
            )?;
        } else {
            let e = self.parse_expression()?;
            let (scope, state) = self.sa();
            let reg = scope.expr_to_any_reg(&mut state.alloc, e, line)?;
            scope.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, reg, 2, 0),
                line,
            )?;
        }
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn parse_if(&mut self) -> Result<(), CompileError> {
        let cond = self.parse_expression()?;
        let line = self.previous.line;
        let else_jump = self.emit_condition_jump(cond, line)?;
        self.parse_block()?;
        if self.match_tok(TokenKind::KwElse) {
            let end_jump = {
                let (scope, state) = self.sa();
                scope.emit_jmp_link(&mut state.alloc, NO_JUMP, line)?
            };
            {
                let (scope, _) = self.sa();
                scope.patch_jump_list_to_here(else_jump);
            }
            if self.check(TokenKind::KwIf) {
                self.advance();
                self.parse_if()?;
            } else {
                self.parse_block()?;
            }
            let (scope, _) = self.sa();
            scope.patch_jump_list_to_here(end_jump);
        } else {
            let (scope, _) = self.sa();
            scope.patch_jump_list_to_here(else_jump);
        }
        Ok(())
    }

    fn parse_while(&mut self) -> Result<(), CompileError> {
        let loop_start = self.scopes.last().unwrap().pc();
        let cond = self.parse_expression()?;
        let line = self.previous.line;
        let exit_jump = self.emit_condition_jump(cond, line)?;
        {
            let scope = self.scopes.last_mut().unwrap();
            let first_local = scope.free_reg;
            scope.loops.push(LoopScope {
                continue_target_pc: loop_start,
                break_jump_list: NO_JUMP,
                scope_depth: scope.scope_depth,
                first_local_reg: first_local,
            });
        }
        self.parse_block()?;
        {
            let (scope, state) = self.sa();
            let back = scope.emit_jmp_link(&mut state.alloc, NO_JUMP, line)?;
            let offset = loop_start as i32 - (back as i32 + 1);
            unsafe {
                (*(*scope.proto).code.add(back as usize)).patch_sj(offset);
            }
            scope.patch_jump_list_to_here(exit_jump);
            if let Some(lp) = scope.loops.pop() {
                scope.patch_jump_list_to_here(lp.break_jump_list);
            }
        }
        Ok(())
    }

    fn emit_condition_jump(&mut self, mut cond: ExprDesc, line: u32) -> Result<u32, CompileError> {
        if matches!(cond.kind, ExprKind::Jump { .. }) {
            let (scope, _) = self.sa();
            scope.invert_condition(&mut cond);
            return Ok(cond.patch_true);
        }
        let (scope, state) = self.sa();
        let reg = scope.expr_to_any_reg(&mut state.alloc, cond, line)?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Test, reg, 0, 0),
            line,
        )?;
        scope.emit_jmp_link(&mut state.alloc, NO_JUMP, line)
    }

    fn parse_expression(&mut self) -> Result<ExprDesc, CompileError> {
        self.parse_precedence(0)
    }

    fn parse_precedence(&mut self, min_bp: u8) -> Result<ExprDesc, CompileError> {
        self.advance();
        let mut left = self.prefix()?;
        loop {
            let (lbp, rbp) = self.current.kind.binding_power();
            if lbp == 0 || lbp < min_bp {
                break;
            }
            if self.current.kind == TokenKind::Assign {
                left = self.parse_assign(left)?;
                continue;
            }
            if self.current.kind == TokenKind::LParen {
                left = self.parse_call(left)?;
                continue;
            }
            if self.current.kind == TokenKind::Dot {
                left = self.parse_field(left)?;
                continue;
            }
            if self.current.kind == TokenKind::LBracket {
                left = self.parse_index(left)?;
                continue;
            }
            let op_tok = self.current.kind;
            let line = self.current.line;
            self.advance();
            let right = self.parse_precedence(rbp)?;
            left = self.infix(op_tok, left, right, line)?;
        }
        Ok(left)
    }

    fn prefix(&mut self) -> Result<ExprDesc, CompileError> {
        let tok = self.previous;
        let line = tok.line;
        match tok.kind {
            TokenKind::Int => {
                let v = parse_int_lexeme(tok.lexeme).map_err(|e| CompileError::new(e, line))?;
                let (scope, state) = self.sa();
                let kind = scope.value_to_expr_kind(&mut state.alloc, v, line)?;
                Ok(ExprDesc::new(kind))
            }
            TokenKind::Float => {
                let cleaned: String = tok.lexeme.chars().filter(|c| *c != '_').collect();
                let f: f64 = cleaned
                    .parse()
                    .map_err(|_| CompileError::new("Invalid float literal", line))?;
                let (scope, state) = self.sa();
                let kind = scope.value_to_expr_kind(&mut state.alloc, Value::from_f64(f), line)?;
                Ok(ExprDesc::new(kind))
            }
            TokenKind::String => {
                let raw = Self::string_token_inner_slice(tok.lexeme);
                let owned = unescape_string(raw, &mut self.scratch)
                    .map_err(|e| CompileError::new(e, line))?
                    .to_vec();
                let (scope, state) = self.sa();
                let s = state
                    .strings
                    .new_string(&mut state.alloc, &mut state.all_objects, 0, &owned)
                    .ok_or_else(|| CompileError::new("Out of memory", line))?;
                let v = Value::from_obj(s.cast(), ObjType::String);
                let kind = scope.value_to_expr_kind(&mut state.alloc, v, line)?;
                Ok(ExprDesc::new(kind))
            }
            TokenKind::KwNil => Ok(ExprDesc::new(ExprKind::Nil)),
            TokenKind::KwTrue => Ok(ExprDesc::new(ExprKind::True)),
            TokenKind::KwFalse => Ok(ExprDesc::new(ExprKind::False)),
            TokenKind::Ident => {
                let state = unsafe { &mut *self.state };
                let kind = resolve_variable(&mut state.alloc, &mut self.scopes, tok.lexeme, line)?;
                Ok(ExprDesc::new(kind))
            }
            TokenKind::LParen => {
                let e = self.parse_expression()?;
                self.consume(TokenKind::RParen, "Expected ')'")?;
                Ok(e)
            }
            TokenKind::Minus => {
                let rhs = self.parse_precedence(TokenKind::Minus.prefix_binding_power())?;
                let (scope, state) = self.sa();
                scope.compile_unop(&mut state.alloc, UnOp::Neg, rhs, line)
            }
            TokenKind::KwNot => {
                let rhs = self.parse_precedence(TokenKind::KwNot.prefix_binding_power())?;
                let (scope, state) = self.sa();
                scope.compile_unop(&mut state.alloc, UnOp::Not, rhs, line)
            }
            TokenKind::Tilde => {
                let rhs = self.parse_precedence(TokenKind::Tilde.prefix_binding_power())?;
                let (scope, state) = self.sa();
                scope.compile_unop(&mut state.alloc, UnOp::BNot, rhs, line)
            }
            other => Err(CompileError::new(
                format!("Unexpected token in expression: {other:?}"),
                line,
            )),
        }
    }

    fn infix(
        &mut self,
        op: TokenKind,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        let bin = match op {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            TokenKind::StarStar => BinOp::Pow,
            TokenKind::Amp => BinOp::Band,
            TokenKind::Pipe => BinOp::Bor,
            TokenKind::Caret => BinOp::Bxor,
            TokenKind::Shl => BinOp::Shl,
            TokenKind::Shr => BinOp::Shr,
            TokenKind::Eq => BinOp::Eq,
            TokenKind::Ne => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Le => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::Ge => BinOp::Ge,
            TokenKind::KwAnd => return self.compile_and(left, right, line),
            TokenKind::KwOr => return self.compile_or(left, right, line),
            _ => {
                return Err(CompileError::new(
                    format!("Unsupported operator {op:?}"),
                    line,
                ));
            }
        };
        let (scope, state) = self.sa();
        scope.compile_binop(&mut state.alloc, bin, left, right, line)
    }

    fn compile_and(
        &mut self,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        let (scope, state) = self.sa();
        let dst = scope.alloc_regs(1, line)?;
        scope.discharge_to_reg(&mut state.alloc, left, dst, line)?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Test, dst, 0, 0),
            line,
        )?;
        let skip = scope.emit_jmp_link(&mut state.alloc, NO_JUMP, line)?;
        scope.discharge_to_reg(&mut state.alloc, right, dst, line)?;
        scope.patch_jump_list_to_here(skip);
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    fn compile_or(
        &mut self,
        left: ExprDesc,
        right: ExprDesc,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        let (scope, state) = self.sa();
        let dst = scope.alloc_regs(1, line)?;
        scope.discharge_to_reg(&mut state.alloc, left, dst, line)?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Test, dst, 0, 1),
            line,
        )?;
        let skip = scope.emit_jmp_link(&mut state.alloc, NO_JUMP, line)?;
        scope.discharge_to_reg(&mut state.alloc, right, dst, line)?;
        scope.patch_jump_list_to_here(skip);
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    fn parse_assign(&mut self, left: ExprDesc) -> Result<ExprDesc, CompileError> {
        let line = self.current.line;
        self.advance(); // '='
        let right = self.parse_precedence(TokenKind::Assign.binding_power().1)?;
        match left.kind {
            ExprKind::Local { reg, is_const } => {
                if is_const {
                    return Err(CompileError::new("Cannot assign to const variable", line));
                }
                let (scope, state) = self.sa();
                scope.discharge_to_reg(&mut state.alloc, right, reg, line)?;
                Ok(ExprDesc::new(ExprKind::Local {
                    reg,
                    is_const: false,
                }))
            }
            ExprKind::Upvalue { idx, is_const } => {
                if is_const {
                    return Err(CompileError::new("Cannot assign to const variable", line));
                }
                let (scope, state) = self.sa();
                let reg = scope.expr_to_any_reg(&mut state.alloc, right, line)?;
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::SetUpval, reg, idx, 0),
                    line,
                )?;
                Ok(ExprDesc::new(ExprKind::NonReloc { reg }))
            }
            ExprKind::Indexed { table_reg, key_reg } => {
                let (scope, state) = self.sa();
                let val = scope.expr_to_any_reg(&mut state.alloc, right, line)?;
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::SetTab, table_reg, key_reg, val),
                    line,
                )?;
                Ok(ExprDesc::new(ExprKind::NonReloc { reg: val }))
            }
            ExprKind::Field { table_reg, key_k } => {
                let (scope, state) = self.sa();
                let val = scope.expr_to_any_reg(&mut state.alloc, right, line)?;
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::SetField, table_reg, key_k, val),
                    line,
                )?;
                Ok(ExprDesc::new(ExprKind::NonReloc { reg: val }))
            }
            _ => Err(CompileError::new("Invalid assignment target", line)),
        }
    }

    fn parse_call(&mut self, callee: ExprDesc) -> Result<ExprDesc, CompileError> {
        let line = self.current.line;
        self.consume(TokenKind::LParen, "Expected '('")?;
        let base = {
            let (scope, state) = self.sa();
            scope.reset_regs();
            let base = scope.free_reg;
            let _ = scope.alloc_regs(1, line)?;
            scope.discharge_to_reg(&mut state.alloc, callee, base, line)?;
            base
        };
        let mut nargs: u8 = 0;
        if !self.check(TokenKind::RParen) {
            loop {
                let arg = self.parse_expression()?;
                let (scope, state) = self.sa();
                let reg = scope.free_reg;
                let _ = scope.alloc_regs(1, line)?;
                scope.discharge_to_reg(&mut state.alloc, arg, reg, line)?;
                nargs = nargs
                    .checked_add(1)
                    .ok_or_else(|| CompileError::new("Too many arguments", line))?;
                if !self.match_tok(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume(TokenKind::RParen, "Expected ')'")?;
        let (scope, state) = self.sa();
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, base, nargs + 1, 2),
            line,
        )?;
        scope.free_reg = base + 1;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: base }))
    }

    fn parse_field(&mut self, table: ExprDesc) -> Result<ExprDesc, CompileError> {
        let line = self.current.line;
        self.consume(TokenKind::Dot, "Expected '.'")?;
        self.consume(TokenKind::Ident, "Expected field name")?;
        let field = self.previous.lexeme;
        let (scope, state) = self.sa();
        let table_reg = scope.expr_to_any_reg(&mut state.alloc, table, line)?;
        let key_k = ensure_string_constant(
            scope,
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            field.as_bytes(),
            line,
        )?;
        let key_u8 =
            u8::try_from(key_k).map_err(|_| CompileError::new("field name K index > 255", line))?;
        Ok(ExprDesc::new(ExprKind::Field {
            table_reg,
            key_k: key_u8,
        }))
    }

    fn parse_index(&mut self, table: ExprDesc) -> Result<ExprDesc, CompileError> {
        let line = self.current.line;
        self.consume(TokenKind::LBracket, "Expected '['")?;
        let key = self.parse_expression()?;
        self.consume(TokenKind::RBracket, "Expected ']'")?;
        let (scope, state) = self.sa();
        let table_reg = scope.expr_to_any_reg(&mut state.alloc, table, line)?;
        let key_reg = scope.expr_to_any_reg(&mut state.alloc, key, line)?;
        Ok(ExprDesc::new(ExprKind::Indexed { table_reg, key_reg }))
    }

    fn string_token_inner_slice(lexeme: &'a str) -> &'a str {
        let b = lexeme.as_bytes();
        if b.len() >= 6 && b.starts_with(b"\"\"\"") && b.ends_with(b"\"\"\"") {
            &lexeme[3..lexeme.len() - 3]
        } else if b.len() >= 2 {
            &lexeme[1..lexeme.len() - 1]
        } else {
            ""
        }
    }

    fn advance(&mut self) {
        self.previous = self.current;
        self.current = self.lexer.next_token();
    }

    fn check(&self, kind: TokenKind) -> bool {
        self.current.kind == kind
    }

    fn match_tok(&mut self, kind: TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume(&mut self, kind: TokenKind, msg: &str) -> Result<(), CompileError> {
        if self.check(kind) {
            self.advance();
            Ok(())
        } else {
            Err(CompileError::new(msg, self.current.line))
        }
    }
}

fn parse_int_lexeme(lexeme: &str) -> Result<Value, &'static str> {
    let cleaned: String = lexeme.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, rest)
    } else {
        (10, cleaned.as_str())
    };
    let v = i64::from_str_radix(digits, radix).map_err(|_| "Invalid integer literal")?;
    if Value::fits_i48(v) {
        Ok(Value::from_i48(v))
    } else {
        Err("Integer literal out of i48 range")
    }
}

impl<'a> FunctionScope<'a> {
    /// Resets `free_reg` to one past the highest live local register.
    pub fn reset_regs(&mut self) {
        let mut top = 0u8;
        for loc in &self.locals {
            if loc.depth >= 0 && loc.reg != 0xFF {
                top = top.max(loc.reg.saturating_add(1));
            }
        }
        self.free_reg = top;
    }
}
