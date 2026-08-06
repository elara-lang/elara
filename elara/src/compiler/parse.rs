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
    lookahead: Option<Token<'a>>,
    scopes: Vec<FunctionScope<'a>>,
    scratch: Vec<u8>,
    /// When true, top-level `export` declarations are accepted.
    is_module: bool,
}

impl<'a> Parser<'a> {
    pub fn new(state: &mut ElaraState, source: &'a str, root: FunctionScope<'a>) -> Self {
        Self::new_inner(state, source, root, false)
    }

    pub fn new_module(state: &mut ElaraState, source: &'a str, root: FunctionScope<'a>) -> Self {
        Self::new_inner(state, source, root, true)
    }

    fn new_inner(
        state: &mut ElaraState,
        source: &'a str,
        root: FunctionScope<'a>,
        is_module: bool,
    ) -> Self {
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
            lookahead: None,
            scopes: vec![root],
            scratch: Vec::new(),
            is_module,
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
        if self.is_module && self.match_tok(TokenKind::KwExport) {
            self.parse_export()
        } else if self.match_tok(TokenKind::KwLet) {
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
        } else if self.match_tok(TokenKind::KwFor) {
            self.parse_for()
        } else if self.match_tok(TokenKind::KwDefer) {
            self.parse_defer()
        } else if self.match_tok(TokenKind::KwYield) {
            self.parse_yield_stmt()
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
        let mut names = vec![self.previous.lexeme];
        let line = self.previous.line;
        while self.match_tok(TokenKind::Comma) {
            self.consume(TokenKind::Ident, "Expected variable name")?;
            names.push(self.previous.lexeme);
        }
        self.consume(TokenKind::Assign, "Expected '=' after variable name")?;
        if names.len() == 1 {
            let init = self.parse_expression()?;
            let (scope, state) = self.sa();
            let reg = scope.declare_local(names[0], is_const, false, line)?;
            scope.discharge_to_reg(&mut state.alloc, init, reg, line)?;
            scope.mark_initialized();
        } else {
            self.parse_multi_let(&names, is_const, line)?;
        }
        let (scope, _) = self.sa();
        scope.reset_regs();
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    /// `let a, b = call(...)` — expands call results into consecutive locals.
    fn parse_multi_let(
        &mut self,
        names: &[&'a str],
        is_const: bool,
        line: u32,
    ) -> Result<(), CompileError> {
        let n = names.len();
        if n > 15 {
            return Err(CompileError::new("Too many variables in let", line));
        }
        let base = {
            let (scope, _) = self.sa();
            let mut first = 0u8;
            for (i, name) in names.iter().enumerate() {
                let reg = scope.declare_local(name, is_const, false, line)?;
                if i == 0 {
                    first = reg;
                }
            }
            // Reclaim registers so the RHS call deposits results into the locals.
            scope.free_reg = first;
            first
        };
        let init = self.parse_expression()?;
        let (scope, state) = self.sa();
        match init.kind {
            ExprKind::NonReloc { reg } if reg == base => unsafe {
                let p = &mut *scope.proto;
                if p.code_count > 0 {
                    let last = &mut *p.code.add(p.code_count as usize - 1);
                    if last.opcode_raw() == OpCode::Call as u8 && last.a() == base {
                        *last = Instruction::make_abc(
                            OpCode::Call,
                            base,
                            last.b(),
                            (n as u8).saturating_add(1),
                        );
                    }
                }
            },
            _ => {
                scope.discharge_to_reg(&mut state.alloc, init, base, line)?;
            }
        }
        let depth = scope.scope_depth as i16;
        let start = scope.locals.len().saturating_sub(n);
        for local in &mut scope.locals[start..] {
            local.depth = depth;
        }
        scope.free_reg = base.saturating_add(n as u8);
        Ok(())
    }

    fn parse_export(&mut self) -> Result<(), CompileError> {
        let line = self.current.line;
        if self.match_tok(TokenKind::KwLet) {
            self.parse_export_binding(false)
        } else if self.match_tok(TokenKind::KwConst) {
            self.parse_export_binding(true)
        } else if self.match_tok(TokenKind::KwFn) {
            self.parse_export_fn()
        } else if self.match_tok(TokenKind::Ident) {
            let name = self.previous.lexeme;
            let line = self.previous.line;
            let value_reg = if self.match_tok(TokenKind::Assign) {
                let init = self.parse_expression()?;
                let (scope, state) = self.sa();
                scope.expr_to_any_reg(&mut state.alloc, init, line)?
            } else {
                let kind = {
                    let state = unsafe { &mut *self.state };
                    resolve_variable(&mut state.alloc, &mut self.scopes, name, line)?
                };
                let (scope, state) = self.sa();
                scope.expr_to_any_reg(&mut state.alloc, ExprDesc::new(kind), line)?
            };
            let (scope, state) = self.sa();
            scope.emit_export(
                &mut state.alloc,
                &mut state.all_objects,
                &mut state.strings,
                name,
                value_reg,
                line,
            )?;
            scope.reset_regs();
            let _ = self.match_tok(TokenKind::Semicolon);
            Ok(())
        } else {
            Err(CompileError::new(
                "Expected let, const, fn, or name after export",
                line,
            ))
        }
    }

    fn parse_export_binding(&mut self, is_const: bool) -> Result<(), CompileError> {
        self.consume(TokenKind::Ident, "Expected variable name")?;
        let name = self.previous.lexeme;
        let line = self.previous.line;
        self.consume(TokenKind::Assign, "Expected '=' after variable name")?;
        let init = self.parse_expression()?;
        let (scope, state) = self.sa();
        let reg = scope.declare_local(name, is_const, false, line)?;
        scope.discharge_to_reg(&mut state.alloc, init, reg, line)?;
        scope.mark_initialized();
        scope.emit_export(
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            name,
            reg,
            line,
        )?;
        scope.reset_regs();
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn parse_export_fn(&mut self) -> Result<(), CompileError> {
        self.consume(TokenKind::Ident, "Expected function name")?;
        let name = self.previous.lexeme;
        let line = self.previous.line;
        let dst = {
            let (scope, _) = self.sa();
            let reg = scope.declare_local(name, true, false, line)?;
            scope.mark_initialized();
            reg
        };
        self.compile_function_body(name, dst)?;
        let (scope, state) = self.sa();
        scope.emit_export(
            &mut state.alloc,
            &mut state.all_objects,
            &mut state.strings,
            name,
            dst,
            line,
        )?;
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
                let pname = self.consume_param_name()?;
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
            // Detect `return f(...)` for possible OP_TAILCALL.
            let e = self.parse_expression()?;
            let (scope, state) = self.sa();
            if let ExprKind::NonReloc { reg } = e.kind {
                // If the last emitted op was CALL at `reg`, rewrite to TailCall when safe.
                let pc = scope.pc();
                if pc > 0 {
                    let prev = unsafe { *(*scope.proto).code.add((pc - 1) as usize) };
                    if prev.opcode_raw() == OpCode::Call as u8 && prev.a() == reg {
                        let b = prev.b();
                        if !scope.has_active_defer() {
                            unsafe {
                                *(*scope.proto).code.add((pc - 1) as usize) =
                                    Instruction::make_abc(OpCode::TailCall, reg, b, 0);
                            }
                            let _ = self.match_tok(TokenKind::Semicolon);
                            return Ok(());
                        }
                    }
                }
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::Return, reg, 2, 0),
                    line,
                )?;
            } else {
                let reg = scope.expr_to_any_reg(&mut state.alloc, e, line)?;
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::Return, reg, 2, 0),
                    line,
                )?;
            }
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
            // Invert so the fused CMP+JMP is taken when the condition is falsy, then return
            // that jump list (now in `patch_false` after the true/false swap).
            scope.invert_condition(&mut cond);
            return Ok(cond.patch_false);
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
            if matches!(
                self.current.kind,
                TokenKind::Assign
                    | TokenKind::PlusAssign
                    | TokenKind::MinusAssign
                    | TokenKind::StarAssign
                    | TokenKind::SlashAssign
                    | TokenKind::PercentAssign
                    | TokenKind::AmpAssign
                    | TokenKind::PipeAssign
                    | TokenKind::CaretAssign
                    | TokenKind::ShlAssign
                    | TokenKind::ShrAssign
            ) {
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
            TokenKind::Ident | TokenKind::KwSelf => {
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
            TokenKind::Hash => {
                let rhs = self.parse_precedence(TokenKind::Hash.prefix_binding_power())?;
                let (scope, state) = self.sa();
                let reg = scope.expr_to_any_reg(&mut state.alloc, rhs, line)?;
                let dst = scope.alloc_regs(1, line)?;
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::Len, dst, reg, 0),
                    line,
                )?;
                Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
            }
            TokenKind::LBrace => self.parse_table_literal(line),
            TokenKind::InterpStart => self.parse_interpolation(tok, line),
            TokenKind::KwFn => self.parse_anon_fn(line),
            other => Err(CompileError::new(
                format!("Unexpected token in expression: {other:?}"),
                line,
            )),
        }
    }

    fn parse_anon_fn(&mut self, line: u32) -> Result<ExprDesc, CompileError> {
        let name = "__anon";
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
                let pname = self.consume_param_name()?;
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
        let mut finished = self.scopes.pop().expect("anon fn scope");
        {
            let state = self.st();
            finished.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                line,
            )?;
        }
        let (scope, state) = self.sa();
        let dst = scope.alloc_regs(1, line)?;
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
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
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
        let op = self.current.kind;
        let line = self.current.line;
        self.advance();
        let right = self.parse_precedence(TokenKind::Assign.binding_power().1)?;
        if op != TokenKind::Assign {
            let bin = match op {
                TokenKind::PlusAssign => BinOp::Add,
                TokenKind::MinusAssign => BinOp::Sub,
                TokenKind::StarAssign => BinOp::Mul,
                TokenKind::SlashAssign => BinOp::Div,
                TokenKind::PercentAssign => BinOp::Mod,
                TokenKind::AmpAssign => BinOp::Band,
                TokenKind::PipeAssign => BinOp::Bor,
                TokenKind::CaretAssign => BinOp::Bxor,
                TokenKind::ShlAssign => BinOp::Shl,
                TokenKind::ShrAssign => BinOp::Shr,
                _ => return Err(CompileError::new("Invalid compound assignment", line)),
            };
            let (scope, state) = self.sa();
            scope.compile_compound_assign(&mut state.alloc, left, bin, right, line)?;
            return Ok(ExprDesc::new(ExprKind::Void));
        }
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
                let dest = base
                    .checked_add(1)
                    .and_then(|b| b.checked_add(nargs))
                    .ok_or_else(|| CompileError::new("Too many arguments", line))?;
                while scope.free_reg <= dest {
                    let _ = scope.alloc_regs(1, line)?;
                }
                scope.discharge_to_reg(&mut state.alloc, arg, dest, line)?;
                scope.free_reg = dest.saturating_add(1);
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

    fn parse_table_literal(&mut self, line: u32) -> Result<ExprDesc, CompileError> {
        // `{` already consumed as prefix previous.
        let (dst, mut builder) = {
            let (scope, state) = self.sa();
            let dst = scope.alloc_regs(1, line)?;
            let builder = scope.begin_table(&mut state.alloc, dst, line)?;
            (dst, builder)
        };
        let mut array_batch = 0u8;
        if !self.check(TokenKind::RBrace) {
            loop {
                if self.check(TokenKind::Ident) && self.check_ahead(TokenKind::Colon) {
                    self.advance();
                    let field = self.previous.lexeme;
                    self.consume(TokenKind::Colon, "Expected ':'")?;
                    let val = self.parse_expression()?;
                    let (scope, state) = self.sa();
                    let key_k = ensure_string_constant(
                        scope,
                        &mut state.alloc,
                        &mut state.all_objects,
                        &mut state.strings,
                        field.as_bytes(),
                        line,
                    )?;
                    let key_u8 = u8::try_from(key_k)
                        .map_err(|_| CompileError::new("field name K index > 255", line))?;
                    let val_reg = scope.expr_to_any_reg(&mut state.alloc, val, line)?;
                    scope.table_setfield(&mut state.alloc, &mut builder, key_u8, val_reg, line)?;
                } else if self.check(TokenKind::LBracket) {
                    self.advance();
                    let key = self.parse_expression()?;
                    self.consume(TokenKind::RBracket, "Expected ']'")?;
                    self.consume(TokenKind::Assign, "Expected '=' in table field")?;
                    let val = self.parse_expression()?;
                    let (scope, state) = self.sa();
                    let key_reg = scope.expr_to_any_reg(&mut state.alloc, key, line)?;
                    let val_reg = scope.expr_to_any_reg(&mut state.alloc, val, line)?;
                    scope.table_settab(&mut state.alloc, &mut builder, key_reg, val_reg, line)?;
                } else {
                    let elem = self.parse_expression()?;
                    let (scope, state) = self.sa();
                    let reg = builder.dst + 1 + array_batch;
                    while scope.free_reg <= reg {
                        let _ = scope.alloc_regs(1, line)?;
                    }
                    scope.discharge_to_reg(&mut state.alloc, elem, reg, line)?;
                    array_batch = array_batch.saturating_add(1);
                    if array_batch == 50 {
                        let base = builder.array_items;
                        scope.table_setlist(
                            &mut state.alloc,
                            &mut builder,
                            array_batch,
                            base,
                            line,
                        )?;
                        array_batch = 0;
                    }
                }
                if !self.match_tok(TokenKind::Comma) {
                    break;
                }
                if self.check(TokenKind::RBrace) {
                    break;
                }
            }
        }
        self.consume(TokenKind::RBrace, "Expected '}'")?;
        let (scope, state) = self.sa();
        if array_batch > 0 {
            let base = builder.array_items;
            scope.table_setlist(&mut state.alloc, &mut builder, array_batch, base, line)?;
        }
        scope.finish_table(builder);
        scope.free_reg = dst + 1;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    fn parse_interpolation(
        &mut self,
        start: Token<'a>,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        // `"prefix {` … `} suffix"` → CONCAT_N of parts (tostring of exprs).
        let mut parts: Vec<ExprDesc> = Vec::new();
        let prefix = Self::interp_prefix_slice(start.lexeme);
        if !prefix.is_empty() {
            parts.push(self.string_const_expr(prefix, line)?);
        }
        loop {
            let expr = self.parse_expression()?;
            // Coerce via tostring if available.
            let coerced = self.call_tostring(expr, line)?;
            parts.push(coerced);
            if self.check(TokenKind::InterpMid) {
                self.advance();
                let mid = Self::interp_mid_slice(self.previous.lexeme);
                if !mid.is_empty() {
                    parts.push(self.string_const_expr(mid, line)?);
                }
                continue;
            }
            if self.check(TokenKind::InterpEnd) {
                self.advance();
                let end = Self::interp_end_slice(self.previous.lexeme);
                if !end.is_empty() {
                    parts.push(self.string_const_expr(end, line)?);
                }
                break;
            }
            return Err(CompileError::new("Unterminated string interpolation", line));
        }
        self.emit_concat_parts(parts, line)
    }

    fn call_tostring(&mut self, e: ExprDesc, line: u32) -> Result<ExprDesc, CompileError> {
        // Materialize tostring(e) when possible; otherwise use value as-is (Concat handles numbers).
        let state = unsafe { &mut *self.state };
        let kind = resolve_variable(&mut state.alloc, &mut self.scopes, "tostring", line);
        let Ok(ts) = kind else {
            return Ok(e);
        };
        let callee = ExprDesc::new(ts);
        let (scope, state) = self.sa();
        let base = scope.free_reg;
        let _ = scope.alloc_regs(1, line)?;
        scope.discharge_to_reg(&mut state.alloc, callee, base, line)?;
        let dest = base + 1;
        while scope.free_reg <= dest {
            let _ = scope.alloc_regs(1, line)?;
        }
        scope.discharge_to_reg(&mut state.alloc, e, dest, line)?;
        scope.free_reg = dest + 1;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, base, 2, 2),
            line,
        )?;
        scope.free_reg = base + 1;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: base }))
    }

    fn emit_concat_parts(
        &mut self,
        parts: Vec<ExprDesc>,
        line: u32,
    ) -> Result<ExprDesc, CompileError> {
        if parts.is_empty() {
            return self.string_const_expr("", line);
        }
        if parts.len() == 1 {
            return Ok(parts.into_iter().next().unwrap());
        }
        let (scope, state) = self.sa();
        let base = scope.free_reg;
        for (i, part) in parts.into_iter().enumerate() {
            let reg = base + i as u8;
            while scope.free_reg <= reg {
                let _ = scope.alloc_regs(1, line)?;
            }
            scope.discharge_to_reg(&mut state.alloc, part, reg, line)?;
            scope.free_reg = reg + 1;
        }
        let n = scope.free_reg - base;
        let dst = base;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::ConcatN, dst, base, n),
            line,
        )?;
        scope.free_reg = dst + 1;
        Ok(ExprDesc::new(ExprKind::NonReloc { reg: dst }))
    }

    fn string_const_expr(&mut self, raw: &str, line: u32) -> Result<ExprDesc, CompileError> {
        let (scope, state) = self.sa();
        let s = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, raw.as_bytes())
            .ok_or_else(|| CompileError::new("Out of memory", line))?;
        let v = Value::from_obj(s.cast(), ObjType::String);
        let kind = scope.value_to_expr_kind(&mut state.alloc, v, line)?;
        Ok(ExprDesc::new(kind))
    }

    fn parse_for(&mut self) -> Result<(), CompileError> {
        self.consume(TokenKind::Ident, "Expected loop variable")?;
        let var = self.previous.lexeme;
        let line = self.previous.line;
        self.consume(TokenKind::KwIn, "Expected 'in'")?;
        // Binding power > range (80) so `0..n` is not consumed as one expression.
        let start = self.parse_precedence(81)?;
        let inclusive = if self.match_tok(TokenKind::DotDotEq) {
            true
        } else {
            self.consume(TokenKind::DotDot, "Expected '..' or '..='")?;
            false
        };
        let end = self.parse_precedence(81)?;
        self.consume(TokenKind::LBrace, "Expected '{'")?;

        // Manual range-for mirroring `compile_range_for`, with body statements parsed here.
        let loop_base = {
            let (scope, state) = self.sa();
            let loop_base = scope.alloc_regs(4, line)?;
            scope.discharge_to_reg(&mut state.alloc, start, loop_base, line)?;
            scope.discharge_to_reg(&mut state.alloc, end, loop_base + 1, line)?;
            if inclusive {
                scope.emit(
                    &mut state.alloc,
                    Instruction::make_abc(OpCode::AddI, loop_base + 1, loop_base + 1, 1u8),
                    line,
                )?;
            }
            scope.emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, loop_base + 2, 1),
                line,
            )?;
            loop_base
        };
        let forprep_pc = {
            let (scope, state) = self.sa();
            scope.emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::ForPrep, loop_base, 0),
                line,
            )?
        };
        {
            let scope = self.scopes.last_mut().unwrap();
            scope.loops.push(LoopScope {
                continue_target_pc: forprep_pc + 1,
                break_jump_list: NO_JUMP,
                scope_depth: scope.scope_depth,
                first_local_reg: loop_base,
            });
            scope.begin_scope();
            scope.locals.push(crate::compiler::scope::Local {
                name: var,
                depth: scope.scope_depth as i16,
                reg: loop_base + 3,
                is_const: false,
                is_captured: false,
                has_close: false,
                inline_const: None,
            });
            scope.free_reg = (loop_base + 4).max(scope.free_reg);
        }
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            self.declaration()?;
            self.scopes.last_mut().unwrap().reset_regs();
        }
        self.consume(TokenKind::RBrace, "Expected '}'")?;
        let line_end = self.previous.line;
        {
            let (scope, state) = self.sa();
            scope.end_scope(&mut state.alloc, line_end)?;
            let forloop_pc = scope.emit(
                &mut state.alloc,
                Instruction::make_asbx(OpCode::ForLoop, loop_base, 0),
                line_end,
            )?;
            let body_pc = forprep_pc + 1;
            let forloop_sbx = body_pc as i32 - (forloop_pc as i32 + 1);
            unsafe {
                *(*scope.proto).code.add(forloop_pc as usize) =
                    Instruction::make_asbx(OpCode::ForLoop, loop_base, forloop_sbx);
            }
            // FORPREP jumps to FORLOOP (which advances and jumps back to body).
            let forprep_sbx = forloop_pc as i32 - (forprep_pc as i32 + 1);
            unsafe {
                *(*scope.proto).code.add(forprep_pc as usize) =
                    Instruction::make_asbx(OpCode::ForPrep, loop_base, forprep_sbx);
            }
            if let Some(lp) = scope.loops.pop() {
                scope.patch_jump_list_to_here(lp.break_jump_list);
            }
        }
        Ok(())
    }

    fn parse_defer(&mut self) -> Result<(), CompileError> {
        let line = self.previous.line;
        // `defer { ... }` → closure + register_close.
        if !self.check(TokenKind::LBrace) {
            return Err(CompileError::new("Expected '{' after defer", line));
        }
        // Build a zero-arg function from the block by reusing compile_function_body shape.
        let name = "__defer";
        let child = {
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
        let (child, child_scope) = child;
        self.scopes.push(child_scope);
        self.parse_block()?;
        let mut finished = self.scopes.pop().expect("defer scope");
        {
            let state = self.st();
            finished.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                line,
            )?;
        }
        let reg = {
            let (scope, state) = self.sa();
            let reg = scope.declare_local(name, true, true, line)?;
            let bx = unsafe {
                (*scope.proto)
                    .add_child_proto(&mut state.alloc, child.as_ptr())
                    .map_err(|e| CompileError::new(e, line))?
            };
            scope.emit(
                &mut state.alloc,
                Instruction::make_abx(OpCode::Closure, reg, bx),
                line,
            )?;
            scope.mark_initialized();
            reg
        };
        // Register close slot via intrinsic `__register_close`.
        self.emit_register_close(reg, line)?;
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn emit_register_close(&mut self, reg: u8, line: u32) -> Result<(), CompileError> {
        let state = unsafe { &mut *self.state };
        let kind = resolve_variable(&mut state.alloc, &mut self.scopes, "__register_close", line)?;
        let (scope, state) = self.sa();
        let base = scope.free_reg;
        let _ = scope.alloc_regs(1, line)?;
        scope.discharge_to_reg(&mut state.alloc, ExprDesc::new(kind), base, line)?;
        let dest = base + 1;
        while scope.free_reg <= dest + 1 {
            let _ = scope.alloc_regs(1, line)?;
        }
        scope.emit_move(&mut state.alloc, dest, reg, line)?;
        scope.emit(
            &mut state.alloc,
            Instruction::make_asbx(OpCode::LoadI, dest + 1, i32::from(reg)),
            line,
        )?;
        scope.free_reg = dest + 2;
        scope.emit(
            &mut state.alloc,
            Instruction::make_abc(OpCode::Call, base, 3, 2),
            line,
        )?;
        scope.reset_regs();
        Ok(())
    }

    fn parse_yield_stmt(&mut self) -> Result<(), CompileError> {
        let line = self.previous.line;
        if self.check(TokenKind::Semicolon)
            || self.check(TokenKind::RBrace)
            || self.check(TokenKind::Eof)
        {
            let (scope, state) = self.sa();
            scope.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Yield, 0, 1, 0),
                line,
            )?;
        } else {
            let e = self.parse_expression()?;
            let (scope, state) = self.sa();
            let reg = scope.expr_to_any_reg(&mut state.alloc, e, line)?;
            scope.emit(
                &mut state.alloc,
                Instruction::make_abc(OpCode::Yield, reg, 2, 0),
                line,
            )?;
        }
        let _ = self.match_tok(TokenKind::Semicolon);
        Ok(())
    }

    fn check_ahead(&mut self, kind: TokenKind) -> bool {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lexer.next_token());
        }
        self.lookahead.unwrap().kind == kind
    }

    fn consume_param_name(&mut self) -> Result<&'a str, CompileError> {
        if self.match_tok(TokenKind::Ident) || self.match_tok(TokenKind::KwSelf) {
            Ok(self.previous.lexeme)
        } else {
            Err(CompileError::new(
                "Expected parameter name",
                self.current.line,
            ))
        }
    }

    fn interp_prefix_slice(lexeme: &'a str) -> &'a str {
        // `"prefix {` or `"""prefix {`
        let b = lexeme.as_bytes();
        if b.starts_with(b"\"\"\"") {
            &lexeme[3..lexeme.len().saturating_sub(1)]
        } else if b.starts_with(b"\"") {
            &lexeme[1..lexeme.len().saturating_sub(1)]
        } else {
            ""
        }
    }

    fn interp_mid_slice(lexeme: &'a str) -> &'a str {
        // `} middle {`
        if lexeme.len() >= 2 {
            &lexeme[1..lexeme.len() - 1]
        } else {
            ""
        }
    }

    fn interp_end_slice(lexeme: &'a str) -> &'a str {
        // `} suffix"` or `} suffix"""`
        let b = lexeme.as_bytes();
        if b.ends_with(b"\"\"\"") {
            &lexeme[1..lexeme.len() - 3]
        } else if b.ends_with(b"\"") {
            &lexeme[1..lexeme.len() - 1]
        } else {
            ""
        }
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
        self.current = if let Some(t) = self.lookahead.take() {
            t
        } else {
            self.lexer.next_token()
        };
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
