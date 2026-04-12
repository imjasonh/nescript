#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::analyzer::VarAllocation;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::parser::ast::*;

/// Code generator: translates AST directly to 6502 instructions.
/// For Milestone 1, we skip the IR and go AST → 6502 directly.
pub struct CodeGen {
    instructions: Vec<Instruction>,
    var_addrs: HashMap<String, u16>,
    const_values: HashMap<String, u16>,
    label_counter: u32,
    /// Address of the NMI-signaled "frame ready" flag in zero page
    pub frame_flag_addr: u8,
    /// Address of controller state byte in zero page
    pub input_addr: u8,
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

        Self {
            instructions: Vec::new(),
            var_addrs,
            const_values,
            label_counter: 0,
            frame_flag_addr: 0x00,
            input_addr: 0x01,
        }
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
        // Generate variable initializers
        for var in &program.globals {
            self.gen_var_init(var);
        }

        // Generate state frame handlers
        // For M1: just generate the main loop for the start state
        for state in &program.states {
            if state.name == program.start_state {
                if let Some(on_frame) = &state.on_frame {
                    // Main loop: wait for frame, run frame handler, repeat
                    let loop_label = self.fresh_label("main_loop");
                    self.emit_label(&loop_label);

                    // Wait for vblank flag
                    let wait_label = self.fresh_label("wait_vblank");
                    self.emit_label(&wait_label);
                    self.emit(LDA, AM::ZeroPage(self.frame_flag_addr));
                    self.emit(BEQ, AM::LabelRelative(wait_label.clone()));
                    // Clear the flag
                    self.emit(LDA, AM::Immediate(0));
                    self.emit(STA, AM::ZeroPage(self.frame_flag_addr));

                    // Generate frame handler body
                    self.gen_block(on_frame);

                    // Jump back to main loop
                    self.emit(JMP, AM::Label(loop_label));
                }
            }
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
                self.emit_label(&loop_label);
                self.gen_block(body);
                self.emit(JMP, AM::Label(loop_label));
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
            Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Return(_, _)
            | Statement::Transition(_, _)
            | Statement::Call(_, _, _) => {
                // TODO: implement for later milestones
            }
            Statement::LoadBackground(_, _) | Statement::SetPalette(_, _) => {
                // TODO: implement in asset pipeline
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
                    }
                }
            }
            LValue::ArrayIndex(_, _) => {
                // TODO: array indexing for later milestones
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
            Expr::ButtonRead(_, button, _) => {
                let mask = button_mask(button);
                self.emit(LDA, AM::ZeroPage(self.input_addr));
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
                self.gen_condition(inner);
                self.emit(EOR, AM::Immediate(0xFF));
                self.emit(AND, AM::Immediate(0x01));
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
            Expr::ButtonRead(_, button, _) => {
                let mask = button_mask(button);
                self.emit(LDA, AM::ZeroPage(self.input_addr));
                self.emit(AND, AM::Immediate(mask));
            }
            Expr::Call(_, _, _) | Expr::ArrayIndex(_, _, _) | Expr::ArrayLiteral(_, _) => {
                // TODO: implement for later milestones
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
            _ => {
                // Mul, Div, Mod, shifts — TODO for later milestones
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
        // OAM buffer is at $0200-$02FF
        // Each sprite entry: Y, tile, attributes, X
        // For M1: sprite name (draw.sprite_name) is parsed but ignored —
        // all draws use OAM slot 0 with tile index 0 (the built-in CHR tile).
        // Sprite name resolution and multiple OAM slots come in M2/M3.
        let _ = &draw.sprite_name;
        // Y position (stored at $0200)
        self.gen_expr(&draw.y);
        self.emit(STA, AM::Absolute(0x0200));

        // Tile index (stored at $0201) — use 0 for default
        if let Some(frame) = &draw.frame {
            self.gen_expr(frame);
        } else {
            self.emit(LDA, AM::Immediate(0));
        }
        self.emit(STA, AM::Absolute(0x0201));

        // Attributes (stored at $0202) — default 0
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::Absolute(0x0202));

        // X position (stored at $0203)
        self.gen_expr(&draw.x);
        self.emit(STA, AM::Absolute(0x0203));
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
