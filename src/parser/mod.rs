pub mod ast;
pub mod preprocess;
#[cfg(test)]
mod tests;

pub use preprocess::preprocess as preprocess_source;

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

    fn peek_at_offset(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens.get(self.pos + offset).map(|t| &t.kind)
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
        let mut functions = Vec::new();
        let mut states = Vec::new();
        let mut sprites = Vec::new();
        let mut palettes = Vec::new();
        let mut backgrounds = Vec::new();
        let mut banks = Vec::new();
        let mut start_state = None;
        let mut on_frame = None;
        let span = self.current_span();

        while *self.peek() != TokenKind::Eof {
            match self.peek().clone() {
                TokenKind::KwGame => {
                    game = Some(self.parse_game_decl()?);
                }
                TokenKind::KwFast | TokenKind::KwSlow => {
                    globals.push(self.parse_var_decl()?);
                }
                TokenKind::KwVar => {
                    globals.push(self.parse_var_decl()?);
                }
                TokenKind::KwFun | TokenKind::KwInline => {
                    functions.push(self.parse_fun_decl()?);
                }
                TokenKind::KwConst => {
                    constants.push(self.parse_const_decl()?);
                }
                TokenKind::KwState => {
                    states.push(self.parse_state_decl()?);
                }
                TokenKind::KwSprite => {
                    sprites.push(self.parse_sprite_decl()?);
                }
                TokenKind::KwPalette => {
                    palettes.push(self.parse_palette_decl()?);
                }
                TokenKind::KwBackground => {
                    backgrounds.push(self.parse_background_decl()?);
                }
                TokenKind::KwBank => {
                    banks.push(self.parse_bank_decl()?);
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
            functions,
            states,
            sprites,
            palettes,
            backgrounds,
            banks,
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
                        "MMC1" => Mapper::MMC1,
                        "UxROM" => Mapper::UxROM,
                        "MMC3" => Mapper::MMC3,
                        _ => {
                            return Err(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("unknown mapper '{val}'"),
                                self.current_span(),
                            )
                            .with_help("supported mappers: NROM, MMC1, UxROM, MMC3"));
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
        let placement = match self.peek() {
            TokenKind::KwFast => {
                self.advance();
                Placement::Fast
            }
            TokenKind::KwSlow => {
                self.advance();
                Placement::Slow
            }
            _ => Placement::Auto,
        };
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
            placement,
            span: Span::new(start.file_id, start.start, self.current_span().end),
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
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    // ── Function declaration ──

    fn parse_fun_decl(&mut self) -> Result<FunDecl, Diagnostic> {
        let start = self.current_span();
        let is_inline = if *self.peek() == TokenKind::KwInline {
            self.advance();
            true
        } else {
            false
        };
        self.expect(&TokenKind::KwFun)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;

        let mut params = Vec::new();
        while *self.peek() != TokenKind::RParen && *self.peek() != TokenKind::Eof {
            let (pname, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ptype = self.parse_type()?;
            params.push(Param {
                name: pname,
                param_type: ptype,
            });
            if *self.peek() == TokenKind::Comma {
                self.advance();
            }
        }
        self.expect(&TokenKind::RParen)?;

        let return_type = if *self.peek() == TokenKind::Arrow {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;

        Ok(FunDecl {
            name,
            params,
            return_type,
            body,
            is_inline,
            span: Span::new(start.file_id, start.start, self.current_span().end),
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
                TokenKind::KwFast | TokenKind::KwSlow | TokenKind::KwVar => {
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
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    // ── Bank declaration ──

    fn parse_bank_decl(&mut self) -> Result<BankDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwBank)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let (type_str, _) = self.expect_ident()?;
        let bank_type = match type_str.as_str() {
            "prg" => BankType::Prg,
            "chr" => BankType::Chr,
            _ => {
                return Err(Diagnostic::error(
                    ErrorCode::E0201,
                    format!("expected 'prg' or 'chr', found '{type_str}'"),
                    self.current_span(),
                ));
            }
        };
        Ok(BankDecl {
            name,
            bank_type,
            span: Span::new(start.file_id, start.start, self.current_span().end),
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

    // ── Sprite / Palette / Background declarations ──

    fn parse_sprite_decl(&mut self) -> Result<SpriteDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwSprite)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut chr_source = None;

        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            let (key, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match key.as_str() {
                "chr" => {
                    chr_source = Some(self.parse_asset_source()?);
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown sprite property '{key}'"),
                        self.current_span(),
                    ));
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        let chr_source = chr_source.ok_or_else(|| {
            Diagnostic::error(ErrorCode::E0201, "sprite requires 'chr' property", start)
        })?;

        Ok(SpriteDecl {
            name,
            chr_source,
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    fn parse_palette_decl(&mut self) -> Result<PaletteDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwPalette)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut colors = None;

        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            let (key, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match key.as_str() {
                "colors" => {
                    self.expect(&TokenKind::LBracket)?;
                    let mut vals = Vec::new();
                    while *self.peek() != TokenKind::RBracket && *self.peek() != TokenKind::Eof {
                        if let TokenKind::IntLiteral(v) = self.peek().clone() {
                            self.advance();
                            vals.push(v as u8);
                        } else {
                            return Err(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("expected color index, found '{}'", self.peek()),
                                self.current_span(),
                            ));
                        }
                        if *self.peek() == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(&TokenKind::RBracket)?;
                    colors = Some(vals);
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown palette property '{key}'"),
                        self.current_span(),
                    ));
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        let colors = colors.ok_or_else(|| {
            Diagnostic::error(
                ErrorCode::E0201,
                "palette requires 'colors' property",
                start,
            )
        })?;

        Ok(PaletteDecl {
            name,
            colors,
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    fn parse_background_decl(&mut self) -> Result<BackgroundDecl, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwBackground)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut chr_source = None;

        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            let (key, _) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            match key.as_str() {
                "chr" => {
                    chr_source = Some(self.parse_asset_source()?);
                }
                _ => {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown background property '{key}'"),
                        self.current_span(),
                    ));
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        let chr_source = chr_source.ok_or_else(|| {
            Diagnostic::error(
                ErrorCode::E0201,
                "background requires 'chr' property",
                start,
            )
        })?;

        Ok(BackgroundDecl {
            name,
            chr_source,
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    fn parse_asset_source(&mut self) -> Result<AssetSource, Diagnostic> {
        match self.peek() {
            TokenKind::At => {
                self.advance(); // consume '@'
                let (kind, _) = self.expect_ident()?;
                self.expect(&TokenKind::LParen)?;
                let path = if let TokenKind::StringLiteral(s) = self.peek().clone() {
                    self.advance();
                    s
                } else {
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("expected string path, found '{}'", self.peek()),
                        self.current_span(),
                    ));
                };
                self.expect(&TokenKind::RParen)?;
                match kind.as_str() {
                    "chr" => Ok(AssetSource::Chr(path)),
                    "binary" => Ok(AssetSource::Binary(path)),
                    _ => Err(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown asset source kind '@{kind}'"),
                        self.current_span(),
                    )),
                }
            }
            TokenKind::LBracket => {
                self.advance();
                let mut bytes = Vec::new();
                while *self.peek() != TokenKind::RBracket && *self.peek() != TokenKind::Eof {
                    if let TokenKind::IntLiteral(v) = self.peek().clone() {
                        self.advance();
                        bytes.push(v as u8);
                    } else {
                        return Err(Diagnostic::error(
                            ErrorCode::E0201,
                            format!("expected byte value, found '{}'", self.peek()),
                            self.current_span(),
                        ));
                    }
                    if *self.peek() == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(AssetSource::Inline(bytes))
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!(
                    "expected asset source (@chr, @binary, or [...]), found '{}'",
                    self.peek()
                ),
                self.current_span(),
            )),
        }
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
            span: Span::new(start.file_id, start.start, self.current_span().end),
        })
    }

    // ── Statements ──

    fn parse_statement(&mut self) -> Result<Statement, Diagnostic> {
        match self.peek().clone() {
            TokenKind::KwFast | TokenKind::KwSlow | TokenKind::KwVar => {
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
            TokenKind::KwLoadBackground => {
                let span = self.current_span();
                self.advance();
                let (name, _) = self.expect_ident()?;
                Ok(Statement::LoadBackground(name, span))
            }
            TokenKind::KwSetPalette => {
                let span = self.current_span();
                self.advance();
                let (name, _) = self.expect_ident()?;
                Ok(Statement::SetPalette(name, span))
            }
            TokenKind::KwScroll => {
                let span = self.current_span();
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let x = self.parse_expr()?;
                self.expect(&TokenKind::Comma)?;
                let y = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Statement::Scroll(x, y, span))
            }
            TokenKind::KwDebug => self.parse_debug_statement(),
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
        // Only consume an ident if it's followed by ':', indicating a keyword arg.
        while matches!(self.peek(), TokenKind::Ident(_))
            && self.peek_at_offset(1) == Some(&TokenKind::Colon)
        {
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

    /// Parse debug.log(...) or debug.assert(...)
    fn parse_debug_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.current_span();
        self.expect(&TokenKind::KwDebug)?;
        self.expect(&TokenKind::Dot)?;
        let (method, _) = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        match method.as_str() {
            "log" => {
                let mut args = Vec::new();
                while *self.peek() != TokenKind::RParen && *self.peek() != TokenKind::Eof {
                    args.push(self.parse_expr()?);
                    if *self.peek() == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RParen)?;
                Ok(Statement::DebugLog(args, start))
            }
            "assert" => {
                let cond = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Statement::DebugAssert(cond, start))
            }
            _ => Err(Diagnostic::error(
                ErrorCode::E0201,
                format!("unknown debug method '{method}' (expected 'log' or 'assert')"),
                start,
            )),
        }
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
            TokenKind::ShiftLeftAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::ShiftLeftAssign,
                    value,
                    start,
                ))
            }
            TokenKind::ShiftRightAssign => {
                self.advance();
                let value = self.parse_expr()?;
                Ok(Statement::Assign(
                    LValue::Var(name),
                    AssignOp::ShiftRightAssign,
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
                while *self.peek() != TokenKind::RParen && *self.peek() != TokenKind::Eof {
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
            TokenKind::AmpAssign => {
                self.advance();
                Ok(AssignOp::AmpAssign)
            }
            TokenKind::PipeAssign => {
                self.advance();
                Ok(AssignOp::PipeAssign)
            }
            TokenKind::CaretAssign => {
                self.advance();
                Ok(AssignOp::CaretAssign)
            }
            TokenKind::ShiftLeftAssign => {
                self.advance();
                Ok(AssignOp::ShiftLeftAssign)
            }
            TokenKind::ShiftRightAssign => {
                self.advance();
                Ok(AssignOp::ShiftRightAssign)
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
        let base = match self.peek().clone() {
            TokenKind::KwU8 => {
                self.advance();
                NesType::U8
            }
            TokenKind::KwI8 => {
                self.advance();
                NesType::I8
            }
            TokenKind::KwU16 => {
                self.advance();
                NesType::U16
            }
            TokenKind::KwBool => {
                self.advance();
                NesType::Bool
            }
            _ => {
                return Err(Diagnostic::error(
                    ErrorCode::E0201,
                    format!("expected type, found '{}'", self.peek()),
                    self.current_span(),
                ));
            }
        };
        // Check for array suffix [N]
        if *self.peek() == TokenKind::LBracket {
            self.advance();
            if let TokenKind::IntLiteral(size) = self.peek().clone() {
                self.advance();
                self.expect(&TokenKind::RBracket)?;
                Ok(NesType::Array(Box::new(base), size))
            } else {
                Err(Diagnostic::error(
                    ErrorCode::E0201,
                    "expected array size",
                    self.current_span(),
                ))
            }
        } else {
            Ok(base)
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
        let expr = match self.peek().clone() {
            TokenKind::Minus => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Expr::UnaryOp(UnaryOp::Negate, Box::new(expr), span)
            }
            TokenKind::KwNot => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Expr::UnaryOp(UnaryOp::Not, Box::new(expr), span)
            }
            TokenKind::Tilde => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary()?;
                Expr::UnaryOp(UnaryOp::BitNot, Box::new(expr), span)
            }
            _ => self.parse_primary()?,
        };
        self.parse_cast_suffix(expr)
    }

    fn parse_cast_suffix(&mut self, mut expr: Expr) -> Result<Expr, Diagnostic> {
        while *self.peek() == TokenKind::KwAs {
            let span = self.current_span();
            self.advance();
            let target_type = self.parse_type()?;
            expr = Expr::Cast(Box::new(expr), target_type, span);
        }
        Ok(expr)
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

                // Check for button.X (player 1 default)
                if name == "button" && *self.peek() == TokenKind::Dot {
                    self.advance();
                    let (button, _) = self.expect_name()?;
                    return Ok(Expr::ButtonRead(None, button, span));
                }

                // Check for p1.button.X / p2.button.X
                if (name == "p1" || name == "p2") && *self.peek() == TokenKind::Dot {
                    self.advance();
                    // Expect 'button'
                    if let TokenKind::Ident(kw) = self.peek().clone() {
                        if kw == "button" {
                            self.advance();
                            self.expect(&TokenKind::Dot)?;
                            let (button, _) = self.expect_name()?;
                            let player = if name == "p1" {
                                Some(Player::P1)
                            } else {
                                Some(Player::P2)
                            };
                            return Ok(Expr::ButtonRead(player, button, span));
                        }
                    }
                    return Err(Diagnostic::error(
                        ErrorCode::E0201,
                        "expected 'button' after 'p1.' or 'p2.'",
                        self.current_span(),
                    ));
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
                    while *self.peek() != TokenKind::RParen && *self.peek() != TokenKind::Eof {
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
            TokenKind::LBracket => {
                let span = self.current_span();
                self.advance();
                let mut elements = Vec::new();
                while *self.peek() != TokenKind::RBracket && *self.peek() != TokenKind::Eof {
                    elements.push(self.parse_expr()?);
                    if *self.peek() == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::ArrayLiteral(elements, span))
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
