pub mod ast;
#[cfg(test)]
mod tests;

use crate::errors::{Diagnostic, ErrorCode};
use crate::lexer::{Span, Token, TokenKind};
use ast::*;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    pub fn parse(mut self) -> (Option<Program>, Vec<Diagnostic>) {
        match self.parse_program() {
            Ok(program) => (Some(program), self.diagnostics),
            Err(diag) => {
                self.diagnostics.push(diag);
                (None, self.diagnostics)
            }
        }
    }

    // ── Token helpers ──

    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map_or(&TokenKind::Eof, |t| &t.kind)
    }

    fn current_span(&self) -> Span {
        self.tokens.get(self.pos).map_or(Span::dummy(), |t| t.span)
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Span, Diagnostic> {
        if self.peek() == expected {
            let span = self.current_span();
            self.advance();
            Ok(span)
        } else {
            Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected '{expected}', found '{}'", self.peek()),
                self.current_span(),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<(String, Span), Diagnostic> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            let span = self.current_span();
            self.advance();
            Ok((name, span))
        } else {
            Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected identifier, found '{}'", self.peek()),
                self.current_span(),
            ))
        }
    }

    /// Accept an identifier or a keyword that can be used as a name
    /// (e.g., button names like "start", "select").
    fn expect_name(&mut self) -> Result<(String, Span), Diagnostic> {
        let span = self.current_span();
        let name = match self.peek() {
            TokenKind::Ident(n) => n.clone(),
            // Keywords that double as button/property names
            TokenKind::KwStart => "start".to_string(),
            TokenKind::KwState => "state".to_string(),
            TokenKind::KwBreak => "break".to_string(),
            TokenKind::KwContinue => "continue".to_string(),
            TokenKind::KwReturn => "return".to_string(),
            _ => {
                return Err(Diagnostic::error(
                    ErrorCode::E0201,
                    format!("expected name, found '{}'", self.peek()),
                    span,
                ));
            }
        };
        self.advance();
        Ok((name, span))
    }

    // ── Program ──

    fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut game = None;
        let mut globals = Vec::new();
        let mut constants = Vec::new();
        let mut states = Vec::new();
        let mut start_state = None;
        let mut on_frame = None;
        let span = self.current_span();

        while *self.peek() != TokenKind::Eof {
            match self.peek().clone() {
                TokenKind::KwGame => {
                    game = Some(self.parse_game_decl()?);
                }
                TokenKind::KwVar => {
                    globals.push(self.parse_var_decl()?);
                }
                TokenKind::KwConst => {
                    constants.push(self.parse_const_decl()?);
                }
                TokenKind::KwState => {
                    states.push(self.parse_state_decl()?);
                }
                TokenKind::KwOn => {
                    // Top-level `on frame` — implicit single state for M1
                    on_frame = Some(self.parse_on_frame()?);
                }
                TokenKind::KwStart => {
                    self.advance();
                    let (name, _) = self.expect_ident()?;
                    start_state = Some(name);
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unexpected token '{}' at top level", self.peek()),
                        self.current_span(),
                    ));
                }
            }
        }

        let game = game.ok_or_else(|| {
            Diagnostic::error(ErrorCode::E0504, "missing 'game' declaration", span)
        })?;

        // If there's a top-level `on frame` but no explicit states,
        // wrap it in an implicit "Main" state
        if !states.is_empty() || on_frame.is_none() {
            // Multi-state or no frame handler — use states as-is
        } else if let Some(frame_block) = on_frame {
            states.push(StateDecl {
                name: "Main".to_string(),
                locals: Vec::new(),
                on_enter: None,
                on_exit: None,
                on_frame: Some(frame_block),
                on_scanline: Vec::new(),
                span,
            });
            if start_state.is_none() {
                start_state = Some("Main".to_string());
            }
        }

        let start_state = start_state.ok_or_else(|| {
            Diagnostic::error(ErrorCode::E0504, "missing 'start' declaration", span)
        })?;

        Ok(Program {
            game,
            globals,
            constants,
            functions: Vec::new(),
            states,
            start_state,
            span,
        })
    }

    // ── Game declaration ──

    fn parse_game_decl(&mut self) -> Result<GameDecl, Diagnostic> {
        let start_span = self.current_span();
        self.expect(&TokenKind::KwGame)?;

        let name = if let TokenKind::StringLiteral(s) = self.peek().clone() {
            self.advance();
            s
        } else {
            return Err(Diagnostic::error(
                ErrorCode::E0201,
                "expected game name string",
                self.current_span(),
            ));
        };

        self.expect(&TokenKind::LBrace)?;

        let mut mapper = Mapper::NROM;
        let mut mirroring = Mirroring::Horizontal;

        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            let (key, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match key.as_str() {
                "mapper" => {
                    let (val, _) = self.expect_ident()?;
                    mapper = match val.as_str() {
                        "NROM" => Mapper::NROM,
                        _ => {
                            return Err(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("unknown mapper '{val}'"),
                                self.current_span(),
                            )
                            .with_help("supported mappers for M1: NROM"));
                        }
                    };
                }
                "mirroring" => {
                    let (val, _) = self.expect_ident()?;
                    mirroring = match val.as_str() {
                        "horizontal" => Mirroring::Horizontal,
                        "vertical" => Mirroring::Vertical,
                        _ => {
                            return Err(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("unknown mirroring '{val}'"),
                                self.current_span(),
                            ));
                        }
                    };
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown game property '{key}'"),
                        self.current_span(),
                    ));
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(GameDecl {
            name,
            mapper,
            mirroring,
            span: Span::new(
                start_span.file_id,
                start_span.start,
                self.current_span().end,
            ),
        })
    }

    // ── Variable declaration ──

    fn parse_var_decl(&mut self) -> Result<VarDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwVar)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let var_type = self.parse_type()?;

        let init = if *self.peek() == TokenKind::Assign {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(VarDecl {
            name,
            var_type,
            init,
            placement: Placement::Auto,
            span: Span::new(start.file_id, start.start, self.current_span().start),
        })
    }

    // ── Const declaration ──

    fn parse_const_decl(&mut self) -> Result<ConstDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwConst)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let const_type = self.parse_type()?;
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;

        Ok(ConstDecl {
            name,
            const_type,
            value,
            span: Span::new(start.file_id, start.start, self.current_span().start),
        })
    }

    // ── State declaration ──

    fn parse_state_decl(&mut self) -> Result<StateDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwState)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut locals = Vec::new();
        let mut on_enter = None;
        let mut on_exit = None;
        let mut on_frame = None;
        let on_scanline = Vec::new();

        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            match self.peek().clone() {
                TokenKind::KwVar => {
                    locals.push(self.parse_var_decl()?);
                }
                TokenKind::KwOn => {
                    self.advance();
                    let (event, _) = self.expect_ident()?;
                    match event.as_str() {
                        "enter" => {
                            on_enter = Some(self.parse_block()?);
                        }
                        "exit" => {
                            on_exit = Some(self.parse_block()?);
                        }
                        "frame" => {
                            on_frame = Some(self.parse_block()?);
                        }
                        _ => {
                            return Err(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("unknown event handler 'on {event}'"),
                                self.current_span(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unexpected token '{}' in state body", self.peek()),
                        self.current_span(),
                    ));
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(StateDecl {
            name,
            locals,
            on_enter,
            on_exit,
            on_frame,
            on_scanline,
            span: Span::new(start.file_id, start.start, self.current_span().start),
        })
    }

    // ── Top-level on frame ──

    fn parse_on_frame(&mut self) -> Result<Block, Diagnostic> {
        self.expect(&TokenKind::KwOn)?;
        let (event, _) = self.expect_ident()?;
        if event != "frame" {
            return Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected 'frame' after 'on', found '{event}'"),
                self.current_span(),
            ));
        }
        self.parse_block()
    }

    // ── Block ──

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::LBrace)?;

        let mut statements = Vec::new();
        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            statements.push(self.parse_statement()?);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Block {
            statements,
            span: Span::new(start.file_id, start.start, self.current_span().start),
        })
    }

    // ── Statements ──

    fn parse_statement(&mut self) -> Result<Statement, Diagnostic> {
        match self.peek().clone() {
            TokenKind::KwVar => {
                let decl = self.parse_var_decl()?;
                Ok(Statement::VarDecl(decl))
            }
            TokenKind::KwIf => self.parse_if(),
            TokenKind::KwWhile => self.parse_while(),
            TokenKind::KwLoop => self.parse_loop(),
            TokenKind::KwBreak => {
                let span = self.current_span();
                self.advance();
                Ok(Statement::Break(span))
            }
            TokenKind::KwContinue => {
                let span = self.current_span();
                self.advance();
                Ok(Statement::Continue(span))
            }
            TokenKind::KwReturn => {
                let span = self.current_span();
                self.advance();
                let value = if *self.peek() == TokenKind::RBrace {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Statement::Return(value, span))
            }
            TokenKind::KwDraw => self.parse_draw(),
            TokenKind::KwTransition => {
                let span = self.current_span();
                self.advance();
                let (name, _) = self.expect_ident()?;
                Ok(Statement::Transition(name, span))
            }
            TokenKind::KwWaitFrame => {
                let span = self.current_span();
                self.advance();
                Ok(Statement::WaitFrame(span))
            }
            TokenKind::Ident(_) => self.parse_assign_or_call(),
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("unexpected token '{}' in statement position", self.peek()),
                self.current_span(),
            )),
        }
    }

    fn parse_if(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwIf)?;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;

        let mut else_ifs = Vec::new();
        let mut else_block = None;

        while *self.peek() == TokenKind::KwElse {
            self.advance();
            if *self.peek() == TokenKind::KwIf {
                self.advance();
                let cond = self.parse_expr()?;
                let block = self.parse_block()?;
                else_ifs.push((cond, block));
            } else {
                else_block = Some(self.parse_block()?);
                break;
            }
        }

        Ok(Statement::If(
            condition, then_block, else_ifs, else_block, start,
        ))
    }

    fn parse_while(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwWhile)?;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Statement::While(condition, body, start))
    }

    fn parse_loop(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwLoop)?;
        let body = self.parse_block()?;
        Ok(Statement::Loop(body, start))
    }

    fn parse_draw(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwDraw)?;
        let (sprite_name, _) = self.expect_ident()?;

        let mut x = None;
        let mut y = None;
        let mut frame = None;

        // Parse keyword arguments: at: (x, y), frame: n
        while let TokenKind::Ident(_) = self.peek() {
            let (key, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match key.as_str() {
                "at" => {
                    self.expect(&TokenKind::LParen)?;
                    x = Some(self.parse_expr()?);
                    self.expect(&TokenKind::Comma)?;
                    y = Some(self.parse_expr()?);
                    self.expect(&TokenKind::RParen)?;
                }
                "frame" => {
                    frame = Some(self.parse_expr()?);
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown draw property '{key}'"),
                        self.current_span(),
                    ));
                }
            }
        }

        let x = x.ok_or_else(|| {
            Diagnostic::error(ErrorCode::E0201, "draw requires 'at: (x, y)'", start)
        })?;
        let y = y.ok_or_else(|| {
            Diagnostic::error(ErrorCode::E0201, "draw requires 'at: (x, y)'", start)
        })?;

        Ok(Statement::Draw(DrawStmt {
            sprite_name,
            x,
            y,
            frame,
            span: start,
        }))
    }

    fn parse_assign_or_call(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        let (name, _) = self.expect_ident()?;

        // Check for button.X pattern
        if name == "button" && *self.peek() == TokenKind::Dot {
            // This shouldn't be a statement on its own
            return Err(Diagnostic::error(
                ErrorCode::E0201,
                "button read is an expression, not a statement",
                start,
            ));
        }

        match self.peek().clone() {
            TokenKind::Assign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::Assign,
                    value,
                    start,
                ))
            }
            TokenKind::PlusAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::PlusAssign,
                    value,
                    start,
                ))
            }
            TokenKind::MinusAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::MinusAssign,
                    value,
                    start,
                ))
            }
            TokenKind::AmpAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::AmpAssign,
                    value,
                    start,
                ))
            }
            TokenKind::PipeAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::PipeAssign,
                    value,
                    start,
                ))
            }
            TokenKind::CaretAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::CaretAssign,
                    value,
                    start,
                ))
            }
            TokenKind::LBracket => {
                // Array index assignment: name[index] = value
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                let op = self.parse_assign_op()?;
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::ArrayIndex(name, Box::new(index)),
                    op,
                    value,
                    start,
                ))
            }
            TokenKind::LParen => {
                // Function call
                self.advance();
                let mut args = Vec::new();
                while *self.peek() != TokenKind::RParen {
                    args.push(self.parse_expr()?);
                    if *self.peek() == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RParen)?;
                Ok(Statement::Call(name, args, start))
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!(
                    "expected assignment operator or '(' after identifier, found '{}'",
                    self.peek()
                ),
                self.current_span(),
            )),
        }
    }

    fn parse_assign_op(&mut self) -> Result<AssignOp, Diagnostic> {
        match self.peek() {
            TokenKind::Assign => {
                self.advance();
                Ok(AssignOp::Assign)
            }
            TokenKind::PlusAssign => {
                self.advance();
                Ok(AssignOp::PlusAssign)
            }
            TokenKind::MinusAssign => {
                self.advance();
                Ok(AssignOp::MinusAssign)
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected assignment operator, found '{}'", self.peek()),
                self.current_span(),
            )),
        }
    }

    // ── Type parsing ──

    fn parse_type(&mut self) -> Result<NesType, Diagnostic> {
        match self.peek().clone() {
            TokenKind::KwU8 => {
                self.advance();
                Ok(NesType::U8)
            }
            TokenKind::KwI8 => {
                self.advance();
                Ok(NesType::I8)
            }
            TokenKind::KwU16 => {
                self.advance();
                Ok(NesType::U16)
            }
            TokenKind::KwBool => {
                self.advance();
                Ok(NesType::Bool)
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected type, found '{}'", self.peek()),
                self.current_span(),
            )),
        }
    }

    // ── Expression parsing (Pratt / precedence climbing) ──

    fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_and_expr()?;
        while *self.peek() == TokenKind::KwOr {
            let span = self.current_span();
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::Or, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_comparison()?;
        while *self.peek() == TokenKind::KwAnd {
            let span = self.current_span();
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::And, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_bitwise_or()?;
        loop {
            let (op, span) = match self.peek() {
                TokenKind::Eq => (BinOp::Eq, self.current_span()),
                TokenKind::NotEq => (BinOp::NotEq, self.current_span()),
                TokenKind::Lt => (BinOp::Lt, self.current_span()),
                TokenKind::Gt => (BinOp::Gt, self.current_span()),
                TokenKind::LtEq => (BinOp::LtEq, self.current_span()),
                TokenKind::GtEq => (BinOp::GtEq, self.current_span()),
                _ => break,
            };
            self.advance();
            let right = self.parse_bitwise_or()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_bitwise_xor()?;
        while *self.peek() == TokenKind::Pipe {
            let span = self.current_span();
            self.advance();
            let right = self.parse_bitwise_xor()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::BitwiseOr, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_bitwise_and()?;
        while *self.peek() == TokenKind::Caret {
            let span = self.current_span();
            self.advance();
            let right = self.parse_bitwise_and()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::BitwiseXor, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_shift()?;
        while *self.peek() == TokenKind::Amp {
            let span = self.current_span();
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::BitwiseAnd, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_additive()?;
        loop {
            let (op, span) = match self.peek() {
                TokenKind::ShiftLeft => (BinOp::ShiftLeft, self.current_span()),
                TokenKind::ShiftRight => (BinOp::ShiftRight, self.current_span()),
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let (op, span) = match self.peek() {
                TokenKind::Plus => (BinOp::Add, self.current_span()),
                TokenKind::Minus => (BinOp::Sub, self.current_span()),
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_unary()?;
        loop {
            let (op, span) = match self.peek() {
                TokenKind::Star => (BinOp::Mul, self.current_span()),
                TokenKind::Slash => (BinOp::Div, self.current_span()),
                TokenKind::Percent => (BinOp::Mod, self.current_span()),
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        match self.peek().clone() {
            TokenKind::Minus => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp(UnaryOp::Negate, Box::new(expr), span))
            }
            TokenKind::KwNot => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp(UnaryOp::Not, Box::new(expr), span))
            }
            TokenKind::Tilde => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp(UnaryOp::BitNot, Box::new(expr), span))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        match self.peek().clone() {
            TokenKind::IntLiteral(v) => {
                let span = self.current_span();
                self.advance();
                Ok(Expr::IntLiteral(v, span))
            }
            TokenKind::BoolLiteral(v) => {
                let span = self.current_span();
                self.advance();
                Ok(Expr::BoolLiteral(v, span))
            }
            TokenKind::Ident(name) => {
                let span = self.current_span();
                self.advance();

                // Check for button.X
                if name == "button" && *self.peek() == TokenKind::Dot {
                    self.advance();
                    let (button, _) = self.expect_name()?;
                    return Ok(Expr::ButtonRead(None, button, span));
                }

                // Check for array index
                if *self.peek() == TokenKind::LBracket {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket)?;
                    return Ok(Expr::ArrayIndex(name, Box::new(index), span));
                }

                // Check for function call
                if *self.peek() == TokenKind::LParen {
                    self.advance();
                    let mut args = Vec::new();
                    while *self.peek() != TokenKind::RParen {
                        args.push(self.parse_expr()?);
                        if *self.peek() == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    return Ok(Expr::Call(name, args, span));
                }

                Ok(Expr::Ident(name, span))
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("expected expression, found '{}'", self.peek()),
                self.current_span(),
            )),
        }
    }
}

pub fn parse(source: &str) -> (Option<Program>, Vec<Diagnostic>) {
    let (tokens, lex_diags) = crate::lexer::lex(source);
    if lex_diags.iter().any(Diagnostic::is_error) {
        return (None, lex_diags);
    }
    let (program, mut parse_diags) = Parser::new(tokens).parse();
    let mut all_diags = lex_diags;
    all_diags.append(&mut parse_diags);
    (program, all_diags)
}
