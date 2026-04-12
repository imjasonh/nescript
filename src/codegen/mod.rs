pub mod ir_codegen;
pub mod peephole;

#[cfg(test)]
mod tests;

pub use ir_codegen::IrCodeGen;

use std::collections::HashMap;

use crate::analyzer::VarAllocation;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::linker::SpriteData;
use crate::parser::ast::*;

/// Zero-page address for the current state index.
pub const ZP_CURRENT_STATE: u8 = 0x03;
/// Zero-page addresses for function call parameter passing ($04-$07, up to 4 params).
pub const ZP_PARAM_BASE: u8 = 0x04;

/// Debug output port address used by Mesen and some other emulators.
pub const DEBUG_PORT: u16 = 0x4800;

/// Code generator: translates AST directly to 6502 instructions.
/// For Milestone 1, we skip the IR and go AST → 6502 directly.
pub struct CodeGen {
    instructions: Vec<Instruction>,
    var_addrs: HashMap<String, u16>,
    const_values: HashMap<String, u16>,
    label_counter: u32,
    /// When true, debug.log/assert statements emit runtime code.
    /// When false, they are stripped entirely.
    debug_mode: bool,
    /// Address of the NMI-signaled "frame ready" flag in zero page
    pub frame_flag_addr: u8,
    /// Address of controller state byte in zero page
    pub input_addr: u8,
    /// Maps state name → numeric index for dispatch
    state_indices: HashMap<String, u8>,
    /// Stack of (`continue_label`, `break_label`) for nested loops
    loop_stack: Vec<(String, String)>,
    /// Next OAM slot to allocate (0-63), reset per frame handler
    next_oam_slot: u8,
    /// Maps sprite name → CHR ROM tile index for `draw SpriteName`
    sprite_tiles: HashMap<String, u8>,
}

impl CodeGen {
    pub fn new(allocations: &[VarAllocation], constants: &[ConstDecl]) -> Self {
        let mut var_addrs = HashMap::new();
        for alloc in allocations {
            var_addrs.insert(alloc.name.clone(), alloc.address);
        }

        let mut const_values = HashMap::new();
        for c in constants {
            if let Expr::IntLiteral(v, _) = &c.value {
                const_values.insert(c.name.clone(), *v);
            }
        }
        // Enum variants get wired in via `with_enums` below so that the
        // main.rs call sites stay concise.

        Self {
            instructions: Vec::new(),
            var_addrs,
            const_values,
            label_counter: 0,
            debug_mode: false,
            frame_flag_addr: 0x00,
            input_addr: 0x01,
            state_indices: HashMap::new(),
            loop_stack: Vec::new(),
            next_oam_slot: 0,
            sprite_tiles: HashMap::new(),
        }
    }

    /// Enable debug mode: debug.log/debug.assert statements will emit runtime code.
    /// When disabled (the default), debug statements are stripped.
    #[must_use]
    pub fn with_debug(mut self, enabled: bool) -> Self {
        self.debug_mode = enabled;
        self
    }

    /// Register sprite-to-tile-index mappings so that `draw SpriteName` can
    /// emit the correct CHR tile index instead of defaulting to 0.
    #[must_use]
    pub fn with_sprites(mut self, sprites: &[SpriteData]) -> Self {
        for sprite in sprites {
            self.sprite_tiles
                .insert(sprite.name.clone(), sprite.tile_index);
        }
        self
    }

    /// Register enum variants as constants (each variant gets a u8
    /// equal to its declaration order within the enum).
    #[must_use]
    pub fn with_enums(mut self, enums: &[EnumDecl]) -> Self {
        for e in enums {
            for (i, (variant, _)) in e.variants.iter().enumerate() {
                self.const_values.insert(variant.clone(), i as u16);
            }
        }
        self
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!("__{prefix}_{}", self.label_counter)
    }

    fn emit(&mut self, opcode: crate::asm::Opcode, mode: AM) {
        self.instructions.push(Instruction::new(opcode, mode));
    }

    fn emit_label(&mut self, name: &str) {
        self.instructions
            .push(Instruction::new(NOP, AM::Label(name.to_string())));
    }

    pub fn generate(mut self, program: &Program) -> Vec<Instruction> {
        // Assign each state an index
        for (i, state) in program.states.iter().enumerate() {
            self.state_indices.insert(state.name.clone(), i as u8);
        }

        // Generate variable initializers
        for var in &program.globals {
            self.gen_var_init(var);
        }

        // Initialize current_state to the start state's index
        let start_index = self
            .state_indices
            .get(&program.start_state)
            .copied()
            .unwrap_or(0);
        self.emit(LDA, AM::Immediate(start_index));
        self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));

        // If the start state has an on_enter handler, call it
        if let Some(start_state) = program
            .states
            .iter()
            .find(|s| s.name == program.start_state)
        {
            if start_state.on_enter.is_some() {
                let enter_label = format!("__state_{start_index}_enter");
                self.emit(JSR, AM::Absolute(0));
                // Patch: use label-based absolute for JSR
                let idx = self.instructions.len() - 1;
                self.instructions[idx] = Instruction::new(JSR, AM::Label(enter_label));
            }
        }

        // Main dispatch loop
        let main_loop_label = "__main_loop".to_string();
        self.emit_label(&main_loop_label);

        // Wait for vblank flag
        let wait_label = "__wait_vblank".to_string();
        self.emit_label(&wait_label);
        self.emit(LDA, AM::ZeroPage(self.frame_flag_addr));
        self.emit(BEQ, AM::LabelRelative(wait_label.clone()));
        // Clear the flag
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::ZeroPage(self.frame_flag_addr));

        // Dispatch based on current_state
        // Uses CMP + BNE skip + JMP pattern to avoid branch range limits
        self.emit(LDA, AM::ZeroPage(ZP_CURRENT_STATE));
        for (i, state) in program.states.iter().enumerate() {
            if state.on_frame.is_some() {
                let frame_label = format!("__state_{i}_frame");
                let skip_label = self.fresh_label("dispatch_skip");
                self.emit(CMP, AM::Immediate(i as u8));
                self.emit(BNE, AM::LabelRelative(skip_label.clone()));
                self.emit(JMP, AM::Label(frame_label));
                self.emit_label(&skip_label);
            }
        }
        self.emit(JMP, AM::Label(main_loop_label.clone()));

        // Generate all state frame handlers as labeled subroutines
        for (i, state) in program.states.iter().enumerate() {
            if let Some(on_frame) = &state.on_frame {
                let frame_label = format!("__state_{i}_frame");
                self.emit_label(&frame_label);
                self.next_oam_slot = 0; // reset OAM allocation per frame
                self.gen_block(on_frame);
                self.emit(JMP, AM::Label(main_loop_label.clone()));
            }
        }

        // Generate on_enter handlers
        for (i, state) in program.states.iter().enumerate() {
            if let Some(on_enter) = &state.on_enter {
                let enter_label = format!("__state_{i}_enter");
                self.emit_label(&enter_label);
                self.gen_block(on_enter);
                self.emit(RTS, AM::Implied);
            }
        }

        // Generate on_exit handlers
        for (i, state) in program.states.iter().enumerate() {
            if let Some(on_exit) = &state.on_exit {
                let exit_label = format!("__state_{i}_exit");
                self.emit_label(&exit_label);
                self.gen_block(on_exit);
                self.emit(RTS, AM::Implied);
            }
        }

        // Generate function bodies
        // We need to clone the function data we need to avoid borrow issues
        let functions: Vec<_> = program
            .functions
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    f.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                    f.body.clone(),
                )
            })
            .collect();
        for (name, params, body) in &functions {
            let fn_label = format!("__fn_{name}");
            self.emit_label(&fn_label);
            // Load parameters from zero-page param slots into local var addresses
            for (j, param_name) in params.iter().enumerate() {
                if let Some(&addr) = self.var_addrs.get(param_name) {
                    self.emit(LDA, AM::ZeroPage(ZP_PARAM_BASE + j as u8));
                    self.emit_store(addr);
                }
            }
            self.gen_block(body);
            self.emit(RTS, AM::Implied); // fallthrough return
        }

        self.instructions
    }

    fn gen_var_init(&mut self, var: &VarDecl) {
        if let Some(init) = &var.init {
            if let Some(&addr) = self.var_addrs.get(&var.name) {
                self.gen_expr(init);
                self.emit_store(addr);
            }
        }
    }

    fn gen_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.gen_statement(stmt);
        }
    }

    fn gen_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::VarDecl(var) => {
                self.gen_var_init(var);
            }
            Statement::Assign(lvalue, op, expr, _) => {
                self.gen_assign(lvalue, *op, expr);
            }
            Statement::If(cond, then_block, else_ifs, else_block, _) => {
                self.gen_if(cond, then_block, else_ifs, else_block.as_ref());
            }
            Statement::While(cond, body, _) => {
                self.gen_while(cond, body);
            }
            Statement::Loop(body, _) => {
                let loop_label = self.fresh_label("loop");
                let end_label = self.fresh_label("loop_end");
                self.loop_stack
                    .push((loop_label.clone(), end_label.clone()));
                self.emit_label(&loop_label);
                self.gen_block(body);
                self.emit(JMP, AM::Label(loop_label));
                self.emit_label(&end_label);
                self.loop_stack.pop();
            }
            Statement::For { .. } => {
                // AST codegen is legacy; for loops are only supported
                // through the IR codegen path. Emit nothing so the
                // program still links — users should use `--use-ast`
                // without for loops if they rely on this path.
            }
            Statement::Draw(draw) => {
                self.gen_draw(draw);
            }
            Statement::WaitFrame(_) => {
                // Wait for vblank flag
                let wait_label = self.fresh_label("wait_frame");
                self.emit_label(&wait_label);
                self.emit(LDA, AM::ZeroPage(self.frame_flag_addr));
                self.emit(BEQ, AM::LabelRelative(wait_label));
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::ZeroPage(self.frame_flag_addr));
            }
            Statement::Break(_) => {
                if let Some((_, break_label)) = self.loop_stack.last() {
                    self.emit(JMP, AM::Label(break_label.clone()));
                }
            }
            Statement::Continue(_) => {
                if let Some((continue_label, _)) = self.loop_stack.last() {
                    self.emit(JMP, AM::Label(continue_label.clone()));
                }
            }
            Statement::Return(value, _) => {
                if let Some(expr) = value {
                    self.gen_expr(expr);
                }
                self.emit(RTS, AM::Implied);
            }
            Statement::Transition(name, _) => {
                if let Some(&idx) = self.state_indices.get(name) {
                    self.emit(LDA, AM::Immediate(idx));
                    self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));
                    // Jump to main loop to start the new state's frame
                    self.emit(JMP, AM::Label("__main_loop".into()));
                }
            }
            Statement::Call(name, args, _) => {
                // Pass arguments via zero-page param slots ($04-$07)
                for (i, arg) in args.iter().enumerate() {
                    self.gen_expr(arg);
                    self.emit(STA, AM::ZeroPage(0x04 + i as u8));
                }
                let fn_label = format!("__fn_{name}");
                self.emit(JSR, AM::Label(fn_label));
            }
            Statement::Scroll(x_expr, y_expr, _) => {
                // PPU scroll register $2005 takes two writes: X then Y
                self.gen_expr(x_expr);
                self.emit(STA, AM::Absolute(0x2005)); // X scroll
                self.gen_expr(y_expr);
                self.emit(STA, AM::Absolute(0x2005)); // Y scroll
            }
            Statement::LoadBackground(_, _) | Statement::SetPalette(_, _) => {
                // TODO: implement in asset pipeline
            }
            Statement::DebugLog(args, _) => {
                if self.debug_mode {
                    for arg in args {
                        self.gen_expr(arg);
                        // Write A to debug port $4800
                        self.emit(STA, AM::Absolute(DEBUG_PORT));
                    }
                }
                // In release mode, stripped entirely
            }
            Statement::DebugAssert(cond, _) => {
                if self.debug_mode {
                    // Evaluate condition; if zero (false), halt
                    self.gen_condition(cond);
                    let pass_label = self.fresh_label("assert_pass");
                    self.emit(BNE, AM::LabelRelative(pass_label.clone()));
                    // Assertion failed: write marker to debug port and BRK
                    self.emit(LDA, AM::Immediate(0xFF));
                    self.emit(STA, AM::Absolute(DEBUG_PORT));
                    self.emit(BRK, AM::Implied);
                    self.emit_label(&pass_label);
                }
            }
            Statement::InlineAsm(body, _) => match crate::asm::parse_inline(body) {
                Ok(parsed) => self.instructions.extend(parsed),
                Err(msg) => {
                    eprintln!("inline asm error: {msg}");
                    self.emit(BRK, AM::Implied);
                }
            },
            Statement::Play(_, _) | Statement::StartMusic(_, _) | Statement::StopMusic(_) => {
                // Audio statements compile to no-ops for now.
            }
        }
    }

    fn gen_assign(&mut self, lvalue: &LValue, op: AssignOp, expr: &Expr) {
        match lvalue {
            LValue::Var(name) => {
                if let Some(&addr) = self.var_addrs.get(name) {
                    match op {
                        AssignOp::Assign => {
                            self.gen_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::PlusAssign => {
                            self.emit_load(addr);
                            self.emit(CLC, AM::Implied);
                            self.gen_adc_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::MinusAssign => {
                            self.emit_load(addr);
                            self.emit(SEC, AM::Implied);
                            self.gen_sbc_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::AmpAssign => {
                            self.emit_load(addr);
                            self.gen_and_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::PipeAssign => {
                            self.emit_load(addr);
                            self.gen_ora_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::CaretAssign => {
                            self.emit_load(addr);
                            self.gen_eor_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::ShiftLeftAssign => {
                            // x <<= n: load, shift left n times, store
                            // For non-constant shift count, emit ASL A once
                            // (matches codegen of the << operator)
                            self.emit_load(addr);
                            self.emit(ASL, AM::Accumulator);
                            let _ = expr; // count is evaluated but not used for dynamic shifts yet
                            self.emit_store(addr);
                        }
                        AssignOp::ShiftRightAssign => {
                            self.emit_load(addr);
                            self.emit(LSR, AM::Accumulator);
                            let _ = expr;
                            self.emit_store(addr);
                        }
                    }
                }
            }
            LValue::Field(name, field) => {
                // Treat `name.field` as a regular variable. The
                // analyzer has already synthesized a VarAllocation
                // entry under the name `"struct.field"`.
                let full_name = format!("{name}.{field}");
                if let Some(&addr) = self.var_addrs.get(&full_name) {
                    match op {
                        AssignOp::Assign => {
                            self.gen_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::PlusAssign => {
                            self.emit_load(addr);
                            self.emit(CLC, AM::Implied);
                            self.gen_adc_expr(expr);
                            self.emit_store(addr);
                        }
                        AssignOp::MinusAssign => {
                            self.emit_load(addr);
                            self.emit(SEC, AM::Implied);
                            self.gen_sbc_expr(expr);
                            self.emit_store(addr);
                        }
                        _ => {
                            // Other compound ops: read, compute, store
                            self.emit_load(addr);
                            self.gen_expr(expr);
                            self.emit_store(addr);
                        }
                    }
                }
            }
            LValue::ArrayIndex(name, index) => {
                if let Some(&base_addr) = self.var_addrs.get(name) {
                    // Evaluate index into X register
                    self.gen_expr(index);
                    self.emit(TAX, AM::Implied);
                    // Evaluate value into A
                    match op {
                        AssignOp::Assign => {
                            self.gen_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::PlusAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.emit(CLC, AM::Implied);
                            self.gen_adc_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::MinusAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.emit(SEC, AM::Implied);
                            self.gen_sbc_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::AmpAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.gen_and_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::PipeAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.gen_ora_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::CaretAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.gen_eor_expr(expr);
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::ShiftLeftAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.emit(ASL, AM::Accumulator);
                            let _ = expr;
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                        AssignOp::ShiftRightAssign => {
                            if base_addr < 0x100 {
                                self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(LDA, AM::AbsoluteX(base_addr));
                            }
                            self.emit(LSR, AM::Accumulator);
                            let _ = expr;
                            if base_addr < 0x100 {
                                self.emit(STA, AM::ZeroPageX(base_addr as u8));
                            } else {
                                self.emit(STA, AM::AbsoluteX(base_addr));
                            }
                        }
                    }
                }
            }
        }
    }

    fn gen_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_ifs: &[(Expr, Block)],
        else_block: Option<&Block>,
    ) {
        let end_label = self.fresh_label("if_end");

        // Evaluate condition
        self.gen_condition(cond);
        let else_label = self.fresh_label("if_else");
        self.emit(BEQ, AM::LabelRelative(else_label.clone()));

        // Then block
        self.gen_block(then_block);
        if !else_ifs.is_empty() || else_block.is_some() {
            self.emit(JMP, AM::Label(end_label.clone()));
        }

        self.emit_label(&else_label);

        // Else-if chains
        for (i, (cond, block)) in else_ifs.iter().enumerate() {
            self.gen_condition(cond);
            let next_label = if i + 1 < else_ifs.len() || else_block.is_some() {
                self.fresh_label("elif")
            } else {
                end_label.clone()
            };
            self.emit(BEQ, AM::LabelRelative(next_label.clone()));
            self.gen_block(block);
            self.emit(JMP, AM::Label(end_label.clone()));
            self.emit_label(&next_label);
        }

        // Else block
        if let Some(block) = else_block {
            self.gen_block(block);
        }

        self.emit_label(&end_label);
    }

    fn gen_while(&mut self, cond: &Expr, body: &Block) {
        let loop_label = self.fresh_label("while");
        let end_label = self.fresh_label("while_end");

        self.emit_label(&loop_label);
        self.gen_condition(cond);
        self.emit(BEQ, AM::LabelRelative(end_label.clone()));
        self.gen_block(body);
        self.emit(JMP, AM::Label(loop_label));
        self.emit_label(&end_label);
    }

    /// Generate code that evaluates a condition, leaving result in A
    /// (non-zero = true, zero = false).
    fn gen_condition(&mut self, expr: &Expr) {
        match expr {
            Expr::ButtonRead(player, button, _) => {
                let mask = button_mask(button);
                let addr = match player {
                    Some(Player::P2) => 0x08, // ZP_INPUT_P2
                    _ => self.input_addr,     // P1 or default
                };
                self.emit(LDA, AM::ZeroPage(addr));
                self.emit(AND, AM::Immediate(mask));
            }
            Expr::BinaryOp(left, op, right, _) => match op {
                BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                    self.gen_comparison(left, *op, right);
                }
                BinOp::And => {
                    let false_label = self.fresh_label("and_false");
                    let end_label = self.fresh_label("and_end");
                    self.gen_condition(left);
                    self.emit(BEQ, AM::LabelRelative(false_label.clone()));
                    self.gen_condition(right);
                    self.emit(JMP, AM::Label(end_label.clone()));
                    self.emit_label(&false_label);
                    self.emit(LDA, AM::Immediate(0));
                    self.emit_label(&end_label);
                }
                BinOp::Or => {
                    let true_label = self.fresh_label("or_true");
                    let end_label = self.fresh_label("or_end");
                    self.gen_condition(left);
                    self.emit(BNE, AM::LabelRelative(true_label.clone()));
                    self.gen_condition(right);
                    self.emit(JMP, AM::Label(end_label.clone()));
                    self.emit_label(&true_label);
                    self.emit(LDA, AM::Immediate(1));
                    self.emit_label(&end_label);
                }
                _ => {
                    // Treat the expression result as a boolean
                    self.gen_expr(expr);
                }
            },
            Expr::BoolLiteral(v, _) => {
                self.emit(LDA, AM::Immediate(u8::from(*v)));
            }
            Expr::UnaryOp(UnaryOp::Not, inner, _) => {
                // Logical NOT: if condition is nonzero → 0, if zero → 1
                self.gen_condition(inner);
                let true_label = self.fresh_label("not_true");
                let end_label = self.fresh_label("not_end");
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                // Condition was true (nonzero), result is false (0)
                self.emit(LDA, AM::Immediate(0));
                self.emit(JMP, AM::Label(end_label.clone()));
                // Condition was false (zero), result is true (1)
                self.emit_label(&true_label);
                self.emit(LDA, AM::Immediate(1));
                self.emit_label(&end_label);
            }
            _ => {
                self.gen_expr(expr);
            }
        }
    }

    fn gen_comparison(&mut self, left: &Expr, op: BinOp, right: &Expr) {
        self.gen_expr(left);
        // Save A to a temp location
        self.emit(PHA, AM::Implied);
        self.gen_expr(right);
        // Transfer right to temp, restore left to A
        self.emit(STA, AM::ZeroPage(0x02)); // temp
        self.emit(PLA, AM::Implied);
        self.emit(CMP, AM::ZeroPage(0x02));

        // Set A based on comparison result
        let true_label = self.fresh_label("cmp_true");
        let end_label = self.fresh_label("cmp_end");

        match op {
            BinOp::Eq => {
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
            }
            BinOp::NotEq => {
                self.emit(BNE, AM::LabelRelative(true_label.clone()));
            }
            BinOp::Lt => {
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
            }
            BinOp::GtEq => {
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
            }
            BinOp::Gt => {
                // A > temp: not equal AND carry set
                self.emit(BEQ, AM::LabelRelative(end_label.clone()));
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
            }
            BinOp::LtEq => {
                // A <= temp: equal OR carry clear
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
            }
            _ => {}
        }
        // False path
        self.emit(LDA, AM::Immediate(0));
        self.emit(JMP, AM::Label(end_label.clone()));
        // True path
        self.emit_label(&true_label);
        self.emit(LDA, AM::Immediate(1));
        self.emit_label(&end_label);
    }

    fn gen_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(LDA, AM::Immediate(*v as u8));
            }
            Expr::BoolLiteral(v, _) => {
                self.emit(LDA, AM::Immediate(u8::from(*v)));
            }
            Expr::Ident(name, _) => {
                if let Some(&value) = self.const_values.get(name) {
                    self.emit(LDA, AM::Immediate(value as u8));
                } else if let Some(&addr) = self.var_addrs.get(name) {
                    self.emit_load(addr);
                }
            }
            Expr::BinaryOp(left, op, right, _) => {
                self.gen_binary_op(left, *op, right);
            }
            Expr::UnaryOp(op, inner, _) => {
                self.gen_expr(inner);
                match op {
                    UnaryOp::Negate => {
                        // Two's complement: EOR #$FF, CLC, ADC #1
                        self.emit(EOR, AM::Immediate(0xFF));
                        self.emit(CLC, AM::Implied);
                        self.emit(ADC, AM::Immediate(1));
                    }
                    UnaryOp::Not => {
                        self.emit(EOR, AM::Immediate(0xFF));
                        self.emit(AND, AM::Immediate(0x01));
                    }
                    UnaryOp::BitNot => {
                        self.emit(EOR, AM::Immediate(0xFF));
                    }
                }
            }
            Expr::ButtonRead(player, button, _) => {
                let mask = button_mask(button);
                let addr = match player {
                    Some(Player::P2) => 0x08, // ZP_INPUT_P2
                    _ => self.input_addr,
                };
                self.emit(LDA, AM::ZeroPage(addr));
                self.emit(AND, AM::Immediate(mask));
            }
            Expr::Cast(inner, _, _) => {
                // For now, just evaluate the inner expression
                self.gen_expr(inner);
            }
            Expr::ArrayIndex(name, index, _) => {
                if let Some(&base_addr) = self.var_addrs.get(name) {
                    self.gen_expr(index);
                    self.emit(TAX, AM::Implied);
                    if base_addr < 0x100 {
                        self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                    } else {
                        self.emit(LDA, AM::AbsoluteX(base_addr));
                    }
                }
            }
            Expr::FieldAccess(name, field, _) => {
                let full_name = format!("{name}.{field}");
                if let Some(&addr) = self.var_addrs.get(&full_name) {
                    self.emit_load(addr);
                }
            }
            Expr::Call(_, _, _) => {
                // Function calls as expressions need JSR — handled elsewhere
                // For now, result is 0
                self.emit(LDA, AM::Immediate(0));
            }
            Expr::ArrayLiteral(_, _) => {
                // Array literals are handled at initialization time
            }
        }
    }

    fn gen_binary_op(&mut self, left: &Expr, op: BinOp, right: &Expr) {
        match op {
            BinOp::Add => {
                self.gen_expr(left);
                self.emit(CLC, AM::Implied);
                self.gen_adc_expr(right);
            }
            BinOp::Sub => {
                self.gen_expr(left);
                self.emit(SEC, AM::Implied);
                self.gen_sbc_expr(right);
            }
            BinOp::BitwiseAnd => {
                self.gen_expr(left);
                self.gen_and_expr(right);
            }
            BinOp::BitwiseOr => {
                self.gen_expr(left);
                self.gen_ora_expr(right);
            }
            BinOp::BitwiseXor => {
                self.gen_expr(left);
                self.gen_eor_expr(right);
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                self.gen_comparison(left, op, right);
            }
            BinOp::Mul => {
                // Software multiply: left in A, right in $02
                self.gen_expr(left);
                self.emit(STA, AM::ZeroPage(0x04)); // save multiplicand
                self.gen_expr(right);
                self.emit(STA, AM::ZeroPage(0x02)); // multiplier
                self.emit(LDA, AM::ZeroPage(0x04)); // restore multiplicand to A
                self.emit(JSR, AM::Label("__multiply".into()));
                // Result is in A
            }
            BinOp::Div => {
                self.gen_expr(left);
                self.emit(STA, AM::ZeroPage(0x04));
                self.gen_expr(right);
                self.emit(STA, AM::ZeroPage(0x02)); // divisor
                self.emit(LDA, AM::ZeroPage(0x04)); // dividend
                self.emit(JSR, AM::Label("__divide".into()));
                // Quotient in A
            }
            BinOp::Mod => {
                self.gen_expr(left);
                self.emit(STA, AM::ZeroPage(0x04));
                self.gen_expr(right);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(LDA, AM::ZeroPage(0x04));
                self.emit(JSR, AM::Label("__divide".into()));
                self.emit(LDA, AM::ZeroPage(0x03)); // remainder is in $03
            }
            BinOp::ShiftLeft => {
                self.gen_expr(left);
                self.emit(ASL, AM::Accumulator);
            }
            BinOp::ShiftRight => {
                self.gen_expr(left);
                self.emit(LSR, AM::Accumulator);
            }
            BinOp::And | BinOp::Or => {
                // Logical operators handled in gen_condition context; here
                // treat as expression evaluation
                self.gen_expr(left);
            }
        }
    }

    /// Generate ADC with an expression (optimizing for immediate values).
    fn gen_adc_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(ADC, AM::Immediate(*v as u8));
            }
            Expr::Ident(name, _) if self.const_values.contains_key(name) => {
                let v = self.const_values[name];
                self.emit(ADC, AM::Immediate(v as u8));
            }
            Expr::Ident(name, _) if self.var_addrs.contains_key(name) => {
                let addr = self.var_addrs[name];
                if addr < 0x100 {
                    self.emit(ADC, AM::ZeroPage(addr as u8));
                } else {
                    self.emit(ADC, AM::Absolute(addr));
                }
            }
            _ => {
                // Complex expr: evaluate, save to temp, then ADC
                self.emit(PHA, AM::Implied);
                self.gen_expr(expr);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(ADC, AM::ZeroPage(0x02));
            }
        }
    }

    /// Generate SBC with an expression.
    fn gen_sbc_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(SBC, AM::Immediate(*v as u8));
            }
            Expr::Ident(name, _) if self.const_values.contains_key(name) => {
                let v = self.const_values[name];
                self.emit(SBC, AM::Immediate(v as u8));
            }
            Expr::Ident(name, _) if self.var_addrs.contains_key(name) => {
                let addr = self.var_addrs[name];
                if addr < 0x100 {
                    self.emit(SBC, AM::ZeroPage(addr as u8));
                } else {
                    self.emit(SBC, AM::Absolute(addr));
                }
            }
            _ => {
                self.emit(PHA, AM::Implied);
                self.gen_expr(expr);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(SBC, AM::ZeroPage(0x02));
            }
        }
    }

    fn gen_and_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(AND, AM::Immediate(*v as u8));
            }
            _ => {
                self.emit(PHA, AM::Implied);
                self.gen_expr(expr);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(AND, AM::ZeroPage(0x02));
            }
        }
    }

    fn gen_ora_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(ORA, AM::Immediate(*v as u8));
            }
            _ => {
                self.emit(PHA, AM::Implied);
                self.gen_expr(expr);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(ORA, AM::ZeroPage(0x02));
            }
        }
    }

    fn gen_eor_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IntLiteral(v, _) => {
                self.emit(EOR, AM::Immediate(*v as u8));
            }
            _ => {
                self.emit(PHA, AM::Implied);
                self.gen_expr(expr);
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(EOR, AM::ZeroPage(0x02));
            }
        }
    }

    fn gen_draw(&mut self, draw: &DrawStmt) {
        // OAM buffer at $0200-$02FF: 64 sprites, 4 bytes each
        // Each entry: Y, tile_index, attributes, X
        let slot = self.next_oam_slot;
        if slot >= 64 {
            return; // OAM full — silently skip (analyzer should warn)
        }
        self.next_oam_slot += 1;
        let base = 0x0200 + u16::from(slot) * 4;

        // Y position
        self.gen_expr(&draw.y);
        self.emit(STA, AM::Absolute(base));

        // Tile index — use frame: expr if provided, else the sprite's
        // resolved tile index, else 0 (default smiley).
        if let Some(frame) = &draw.frame {
            self.gen_expr(frame);
        } else if let Some(&tile_idx) = self.sprite_tiles.get(&draw.sprite_name) {
            self.emit(LDA, AM::Immediate(tile_idx));
        } else {
            self.emit(LDA, AM::Immediate(0));
        }
        self.emit(STA, AM::Absolute(base + 1));

        // Attributes — default 0
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::Absolute(base + 2));

        // X position
        self.gen_expr(&draw.x);
        self.emit(STA, AM::Absolute(base + 3));
    }

    fn emit_load(&mut self, addr: u16) {
        if addr < 0x100 {
            self.emit(LDA, AM::ZeroPage(addr as u8));
        } else {
            self.emit(LDA, AM::Absolute(addr));
        }
    }

    fn emit_store(&mut self, addr: u16) {
        if addr < 0x100 {
            self.emit(STA, AM::ZeroPage(addr as u8));
        } else {
            self.emit(STA, AM::Absolute(addr));
        }
    }
}

/// Map button name to NES controller bit mask.
fn button_mask(button: &str) -> u8 {
    match button {
        "a" => 0x80,
        "b" => 0x40,
        "select" => 0x20,
        "start" => 0x10,
        "up" => 0x08,
        "down" => 0x04,
        "left" => 0x02,
        "right" => 0x01,
        _ => 0x00,
    }
}
