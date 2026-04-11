use std::collections::HashMap;

use super::*;
use crate::analyzer::{AnalysisResult, VarAllocation};
use crate::parser::ast::*;

/// Lower a parsed & analyzed program into IR.
pub fn lower(program: &Program, analysis: &AnalysisResult) -> IrProgram {
    let mut ctx = LoweringContext::new(analysis);
    ctx.lower_program(program);
    ctx.finish()
}

struct LoweringContext {
    functions: Vec<IrFunction>,
    globals: Vec<IrGlobal>,
    rom_data: Vec<IrRomBlock>,
    var_map: HashMap<String, VarId>,
    const_values: HashMap<String, u16>,
    next_var_id: u32,
    next_temp: u32,
    next_block: u32,
    // Current function being built
    current_blocks: Vec<IrBasicBlock>,
    current_ops: Vec<IrOp>,
    current_label: String,
    // Loop context for break/continue
    loop_stack: Vec<LoopContext>,
}

struct LoopContext {
    continue_label: String,
    break_label: String,
}

impl LoweringContext {
    fn new(analysis: &AnalysisResult) -> Self {
        let mut var_map = HashMap::new();
        let mut next_var_id = 0u32;

        // Pre-register all allocated variables
        for alloc in &analysis.var_allocations {
            var_map.insert(alloc.name.clone(), VarId(next_var_id));
            next_var_id += 1;
        }

        Self {
            functions: Vec::new(),
            globals: Vec::new(),
            rom_data: Vec::new(),
            var_map,
            const_values: HashMap::new(),
            next_var_id,
            next_temp: 0,
            next_block: 0,
            current_blocks: Vec::new(),
            current_ops: Vec::new(),
            current_label: String::new(),
            loop_stack: Vec::new(),
        }
    }

    fn fresh_temp(&mut self) -> IrTemp {
        let t = IrTemp(self.next_temp);
        self.next_temp += 1;
        t
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        self.next_block += 1;
        format!("{prefix}_{}", self.next_block)
    }

    fn get_or_create_var(&mut self, name: &str) -> VarId {
        if let Some(&id) = self.var_map.get(name) {
            id
        } else {
            let id = VarId(self.next_var_id);
            self.next_var_id += 1;
            self.var_map.insert(name.to_string(), id);
            id
        }
    }

    fn emit(&mut self, op: IrOp) {
        self.current_ops.push(op);
    }

    fn start_block(&mut self, label: &str) {
        self.current_label = label.to_string();
        self.current_ops = Vec::new();
    }

    fn end_block(&mut self, terminator: IrTerminator) {
        self.current_blocks.push(IrBasicBlock {
            label: self.current_label.clone(),
            ops: std::mem::take(&mut self.current_ops),
            terminator,
        });
    }

    fn finish(self) -> IrProgram {
        IrProgram {
            functions: self.functions,
            globals: self.globals,
            rom_data: self.rom_data,
        }
    }

    fn lower_program(&mut self, program: &Program) {
        // Register constants
        for c in &program.constants {
            if let Expr::IntLiteral(v, _) = &c.value {
                self.const_values.insert(c.name.clone(), *v);
            }
        }

        // Lower globals
        for var in &program.globals {
            let var_id = self.get_or_create_var(&var.name);
            let init = var.init.as_ref().and_then(|e| {
                if let Expr::IntLiteral(v, _) = e {
                    Some(*v)
                } else {
                    None
                }
            });
            self.globals.push(IrGlobal {
                var_id,
                name: var.name.clone(),
                size: type_size(&var.var_type),
                init_value: init,
            });
        }

        // Lower user functions
        for fun in &program.functions {
            self.lower_function(fun);
        }

        // Lower state handlers
        for state in &program.states {
            self.lower_state(state, state.name == program.start_state);
        }
    }

    fn lower_function(&mut self, fun: &FunDecl) {
        self.next_temp = 0;
        self.current_blocks = Vec::new();
        let mut locals = Vec::new();

        // Register parameters as locals
        for param in &fun.params {
            let var_id = self.get_or_create_var(&param.name);
            locals.push(IrLocal {
                var_id,
                name: param.name.clone(),
                size: type_size(&param.param_type),
            });
        }

        let entry = self.fresh_label(&format!("fn_{}_entry", fun.name));
        self.start_block(&entry);
        self.lower_block(&fun.body);

        // Ensure the function ends with a return
        if self.current_ops.is_empty()
            || !matches!(
                self.current_blocks.last().map(|b| &b.terminator),
                Some(IrTerminator::Return(_))
            )
        {
            self.end_block(IrTerminator::Return(None));
        }

        self.functions.push(IrFunction {
            name: fun.name.clone(),
            blocks: std::mem::take(&mut self.current_blocks),
            locals,
            param_count: fun.params.len(),
            has_return: fun.return_type.is_some(),
            source_span: fun.span,
        });
    }

    fn lower_state(&mut self, state: &StateDecl, _is_start: bool) {
        // Lower each event handler as a separate function

        if let Some(on_enter) = &state.on_enter {
            self.lower_handler(&format!("{}_enter", state.name), on_enter, state);
        }

        if let Some(on_exit) = &state.on_exit {
            self.lower_handler(&format!("{}_exit", state.name), on_exit, state);
        }

        if let Some(on_frame) = &state.on_frame {
            self.lower_handler(&format!("{}_frame", state.name), on_frame, state);
        }
    }

    fn lower_handler(&mut self, name: &str, block: &Block, state: &StateDecl) {
        self.next_temp = 0;
        self.current_blocks = Vec::new();
        let mut locals = Vec::new();

        // Register state-local variables
        for var in &state.locals {
            let var_id = self.get_or_create_var(&var.name);
            locals.push(IrLocal {
                var_id,
                name: var.name.clone(),
                size: type_size(&var.var_type),
            });
        }

        let entry = self.fresh_label(&format!("{name}_entry"));
        self.start_block(&entry);
        self.lower_block(block);
        self.end_block(IrTerminator::Return(None));

        self.functions.push(IrFunction {
            name: name.to_string(),
            blocks: std::mem::take(&mut self.current_blocks),
            locals,
            param_count: 0,
            has_return: false,
            source_span: state.span,
        });
    }

    fn lower_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.lower_statement(stmt);
        }
    }

    fn lower_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::VarDecl(var) => {
                if let Some(init) = &var.init {
                    let var_id = self.get_or_create_var(&var.name);
                    let val = self.lower_expr(init);
                    self.emit(IrOp::StoreVar(var_id, val));
                }
            }
            Statement::Assign(lvalue, op, expr, _) => {
                self.lower_assign(lvalue, *op, expr);
            }
            Statement::If(cond, then_block, else_ifs, else_block, _) => {
                self.lower_if(cond, then_block, else_ifs, else_block.as_ref());
            }
            Statement::While(cond, body, _) => {
                self.lower_while(cond, body);
            }
            Statement::Loop(body, _) => {
                self.lower_loop(body);
            }
            Statement::Break(_) => {
                if let Some(ctx) = self.loop_stack.last() {
                    let label = ctx.break_label.clone();
                    self.end_block(IrTerminator::Jump(label.clone()));
                    let cont = self.fresh_label("after_break");
                    self.start_block(&cont);
                }
            }
            Statement::Continue(_) => {
                if let Some(ctx) = self.loop_stack.last() {
                    let label = ctx.continue_label.clone();
                    self.end_block(IrTerminator::Jump(label.clone()));
                    let cont = self.fresh_label("after_continue");
                    self.start_block(&cont);
                }
            }
            Statement::Return(value, _) => {
                let temp = value.as_ref().map(|e| self.lower_expr(e));
                self.end_block(IrTerminator::Return(temp));
                let cont = self.fresh_label("after_return");
                self.start_block(&cont);
            }
            Statement::Draw(draw) => {
                let x = self.lower_expr(&draw.x);
                let y = self.lower_expr(&draw.y);
                let frame = draw.frame.as_ref().map(|e| self.lower_expr(e));
                self.emit(IrOp::DrawSprite {
                    sprite_name: draw.sprite_name.clone(),
                    x,
                    y,
                    frame,
                });
            }
            Statement::Transition(name, _) => {
                self.emit(IrOp::Transition(name.clone()));
            }
            Statement::WaitFrame(_) => {
                self.emit(IrOp::WaitFrame);
            }
            Statement::Call(name, args, _) => {
                let arg_temps: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrOp::Call(None, name.clone(), arg_temps));
            }
        }
    }

    fn lower_assign(&mut self, lvalue: &LValue, op: AssignOp, expr: &Expr) {
        match lvalue {
            LValue::Var(name) => {
                let var_id = self.get_or_create_var(name);
                match op {
                    AssignOp::Assign => {
                        let val = self.lower_expr(expr);
                        self.emit(IrOp::StoreVar(var_id, val));
                    }
                    _ => {
                        let current = self.fresh_temp();
                        self.emit(IrOp::LoadVar(current, var_id));
                        let rhs = self.lower_expr(expr);
                        let result = self.fresh_temp();
                        let ir_op = match op {
                            AssignOp::PlusAssign => IrOp::Add(result, current, rhs),
                            AssignOp::MinusAssign => IrOp::Sub(result, current, rhs),
                            AssignOp::AmpAssign => IrOp::And(result, current, rhs),
                            AssignOp::PipeAssign => IrOp::Or(result, current, rhs),
                            AssignOp::CaretAssign => IrOp::Xor(result, current, rhs),
                            AssignOp::Assign => unreachable!(),
                        };
                        self.emit(ir_op);
                        self.emit(IrOp::StoreVar(var_id, result));
                    }
                }
            }
            LValue::ArrayIndex(name, index) => {
                let var_id = self.get_or_create_var(name);
                let idx = self.lower_expr(index);
                let val = self.lower_expr(expr);
                // For compound assignment on arrays, load first
                if op != AssignOp::Assign {
                    let current = self.fresh_temp();
                    self.emit(IrOp::ArrayLoad(current, var_id, idx));
                    let result = self.fresh_temp();
                    let ir_op = match op {
                        AssignOp::PlusAssign => IrOp::Add(result, current, val),
                        AssignOp::MinusAssign => IrOp::Sub(result, current, val),
                        AssignOp::AmpAssign => IrOp::And(result, current, val),
                        AssignOp::PipeAssign => IrOp::Or(result, current, val),
                        AssignOp::CaretAssign => IrOp::Xor(result, current, val),
                        AssignOp::Assign => unreachable!(),
                    };
                    self.emit(ir_op);
                    self.emit(IrOp::ArrayStore(var_id, idx, result));
                } else {
                    self.emit(IrOp::ArrayStore(var_id, idx, val));
                }
            }
        }
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_ifs: &[(Expr, Block)],
        else_block: Option<&Block>,
    ) {
        let end_label = self.fresh_label("if_end");

        let cond_temp = self.lower_expr(cond);
        let then_label = self.fresh_label("if_then");
        let else_label = if else_ifs.is_empty() && else_block.is_none() {
            end_label.clone()
        } else {
            self.fresh_label("if_else")
        };

        self.end_block(IrTerminator::Branch(
            cond_temp,
            then_label.clone(),
            else_label.clone(),
        ));

        // Then block
        self.start_block(&then_label);
        self.lower_block(then_block);
        self.end_block(IrTerminator::Jump(end_label.clone()));

        // Else-if chains
        let mut current_else = else_label;
        for (i, (elif_cond, elif_block)) in else_ifs.iter().enumerate() {
            self.start_block(&current_else);
            let cond_temp = self.lower_expr(elif_cond);
            let elif_then = self.fresh_label("elif_then");
            let elif_else = if i + 1 < else_ifs.len() || else_block.is_some() {
                self.fresh_label("elif_else")
            } else {
                end_label.clone()
            };
            self.end_block(IrTerminator::Branch(
                cond_temp,
                elif_then.clone(),
                elif_else.clone(),
            ));

            self.start_block(&elif_then);
            self.lower_block(elif_block);
            self.end_block(IrTerminator::Jump(end_label.clone()));

            current_else = elif_else;
        }

        // Else block
        if let Some(block) = else_block {
            self.start_block(&current_else);
            self.lower_block(block);
            self.end_block(IrTerminator::Jump(end_label.clone()));
        }

        self.start_block(&end_label);
    }

    fn lower_while(&mut self, cond: &Expr, body: &Block) {
        let cond_label = self.fresh_label("while_cond");
        let body_label = self.fresh_label("while_body");
        let end_label = self.fresh_label("while_end");

        self.end_block(IrTerminator::Jump(cond_label.clone()));

        // Condition check
        self.start_block(&cond_label);
        let cond_temp = self.lower_expr(cond);
        self.end_block(IrTerminator::Branch(
            cond_temp,
            body_label.clone(),
            end_label.clone(),
        ));

        // Body
        self.loop_stack.push(LoopContext {
            continue_label: cond_label,
            break_label: end_label.clone(),
        });
        self.start_block(&body_label);
        self.lower_block(body);
        let cond_label = &self.loop_stack.last().unwrap().continue_label.clone();
        self.end_block(IrTerminator::Jump(cond_label.clone()));
        self.loop_stack.pop();

        self.start_block(&end_label);
    }

    fn lower_loop(&mut self, body: &Block) {
        let body_label = self.fresh_label("loop_body");
        let end_label = self.fresh_label("loop_end");

        self.end_block(IrTerminator::Jump(body_label.clone()));

        self.loop_stack.push(LoopContext {
            continue_label: body_label.clone(),
            break_label: end_label.clone(),
        });
        self.start_block(&body_label);
        self.lower_block(body);
        self.end_block(IrTerminator::Jump(body_label));
        self.loop_stack.pop();

        self.start_block(&end_label);
    }

    fn lower_expr(&mut self, expr: &Expr) -> IrTemp {
        match expr {
            Expr::IntLiteral(v, _) => {
                let t = self.fresh_temp();
                self.emit(IrOp::LoadImm(t, *v as u8));
                t
            }
            Expr::BoolLiteral(v, _) => {
                let t = self.fresh_temp();
                self.emit(IrOp::LoadImm(t, u8::from(*v)));
                t
            }
            Expr::Ident(name, _) => {
                // Check constants first
                if let Some(&val) = self.const_values.get(name) {
                    let t = self.fresh_temp();
                    self.emit(IrOp::LoadImm(t, val as u8));
                    return t;
                }
                let var_id = self.get_or_create_var(name);
                let t = self.fresh_temp();
                self.emit(IrOp::LoadVar(t, var_id));
                t
            }
            Expr::ArrayIndex(name, index, _) => {
                let var_id = self.get_or_create_var(name);
                let idx = self.lower_expr(index);
                let t = self.fresh_temp();
                self.emit(IrOp::ArrayLoad(t, var_id, idx));
                t
            }
            Expr::BinaryOp(left, op, right, _) => {
                self.lower_binop(left, *op, right)
            }
            Expr::UnaryOp(op, inner, _) => {
                let val = self.lower_expr(inner);
                let t = self.fresh_temp();
                match op {
                    UnaryOp::Negate => self.emit(IrOp::Negate(t, val)),
                    UnaryOp::Not => {
                        // Logical not: compare with 0
                        let zero = self.fresh_temp();
                        self.emit(IrOp::LoadImm(zero, 0));
                        self.emit(IrOp::CmpEq(t, val, zero));
                    }
                    UnaryOp::BitNot => self.emit(IrOp::Complement(t, val)),
                }
                t
            }
            Expr::Call(name, args, _) => {
                let arg_temps: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                let t = self.fresh_temp();
                self.emit(IrOp::Call(Some(t), name.clone(), arg_temps));
                t
            }
            Expr::ButtonRead(_, button, _) => {
                // Button reads are lowered to a ReadInput + mask check
                self.emit(IrOp::ReadInput);
                let t = self.fresh_temp();
                let mask = button_mask(button);
                let mask_temp = self.fresh_temp();
                self.emit(IrOp::LoadImm(mask_temp, mask));
                self.emit(IrOp::And(t, t, mask_temp));
                t
            }
            Expr::ArrayLiteral(_, _) => {
                // Array literals are handled during initialization, not as general expressions
                let t = self.fresh_temp();
                self.emit(IrOp::LoadImm(t, 0));
                t
            }
        }
    }

    fn lower_binop(&mut self, left: &Expr, op: BinOp, right: &Expr) -> IrTemp {
        // Short-circuit for logical operators
        match op {
            BinOp::And => return self.lower_logical_and(left, right),
            BinOp::Or => return self.lower_logical_or(left, right),
            _ => {}
        }

        let l = self.lower_expr(left);
        let r = self.lower_expr(right);
        let t = self.fresh_temp();

        match op {
            BinOp::Add => self.emit(IrOp::Add(t, l, r)),
            BinOp::Sub => self.emit(IrOp::Sub(t, l, r)),
            BinOp::Mul => self.emit(IrOp::Mul(t, l, r)),
            BinOp::BitwiseAnd => self.emit(IrOp::And(t, l, r)),
            BinOp::BitwiseOr => self.emit(IrOp::Or(t, l, r)),
            BinOp::BitwiseXor => self.emit(IrOp::Xor(t, l, r)),
            BinOp::Eq => self.emit(IrOp::CmpEq(t, l, r)),
            BinOp::NotEq => self.emit(IrOp::CmpNe(t, l, r)),
            BinOp::Lt => self.emit(IrOp::CmpLt(t, l, r)),
            BinOp::Gt => self.emit(IrOp::CmpGt(t, l, r)),
            BinOp::LtEq => self.emit(IrOp::CmpLtEq(t, l, r)),
            BinOp::GtEq => self.emit(IrOp::CmpGtEq(t, l, r)),
            BinOp::ShiftLeft => self.emit(IrOp::ShiftLeft(t, l, 1)), // TODO: dynamic shift
            BinOp::ShiftRight => self.emit(IrOp::ShiftRight(t, l, 1)),
            BinOp::Div | BinOp::Mod => {
                // Software div/mod — emit as a call to runtime for now
                self.emit(IrOp::LoadImm(t, 0));
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        }

        t
    }

    fn lower_logical_and(&mut self, left: &Expr, right: &Expr) -> IrTemp {
        let result = self.fresh_temp();
        let right_label = self.fresh_label("and_right");
        let end_label = self.fresh_label("and_end");
        let false_label = self.fresh_label("and_false");

        let l = self.lower_expr(left);
        self.end_block(IrTerminator::Branch(
            l,
            right_label.clone(),
            false_label.clone(),
        ));

        // Right side (only evaluated if left is true)
        self.start_block(&right_label);
        let r = self.lower_expr(right);
        self.emit(IrOp::StoreVar(VarId(self.next_var_id), r)); // temp storage
        self.end_block(IrTerminator::Jump(end_label.clone()));

        // False path
        self.start_block(&false_label);
        self.emit(IrOp::LoadImm(result, 0));
        self.end_block(IrTerminator::Jump(end_label.clone()));

        // Merge
        self.start_block(&end_label);
        result
    }

    fn lower_logical_or(&mut self, left: &Expr, right: &Expr) -> IrTemp {
        let result = self.fresh_temp();
        let right_label = self.fresh_label("or_right");
        let end_label = self.fresh_label("or_end");
        let true_label = self.fresh_label("or_true");

        let l = self.lower_expr(left);
        self.end_block(IrTerminator::Branch(
            l,
            true_label.clone(),
            right_label.clone(),
        ));

        // True path (left was true)
        self.start_block(&true_label);
        self.emit(IrOp::LoadImm(result, 1));
        self.end_block(IrTerminator::Jump(end_label.clone()));

        // Right side
        self.start_block(&right_label);
        let r = self.lower_expr(right);
        self.emit(IrOp::StoreVar(VarId(self.next_var_id), r));
        self.end_block(IrTerminator::Jump(end_label.clone()));

        // Merge
        self.start_block(&end_label);
        result
    }
}

fn type_size(t: &NesType) -> u16 {
    match t {
        NesType::U8 | NesType::I8 | NesType::Bool => 1,
        NesType::U16 => 2,
        NesType::Array(elem, count) => type_size(elem) * count,
    }
}

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
