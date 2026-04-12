use std::collections::HashMap;

use super::*;
use crate::analyzer::AnalysisResult;
use crate::parser::ast::*;

/// Marker prefix the lowering prepends to the body of a `raw asm`
/// block, telling the codegen to skip `{var}` substitution. Uses
/// NUL characters so no normal source text can spoof it.
pub const RAW_ASM_PREFIX: &str = "\0RAW\0";

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
    current_locals: Vec<IrLocal>,
    // Loop context for break/continue
    loop_stack: Vec<LoopContext>,
    // State metadata captured from the AST
    state_names: Vec<String>,
    start_state: String,
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
            current_locals: Vec::new(),
            loop_stack: Vec::new(),
            state_names: Vec::new(),
            start_state: String::new(),
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

    /// Try to evaluate an expression at compile time, using the
    /// already-registered constants as operands. Returns `None` if
    /// the expression references something that isn't known at this
    /// point (e.g. a runtime variable) or contains an operator we
    /// don't constant-fold. The result is a u16 to keep the same
    /// range as the AST integer literal type.
    fn eval_const(&self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::IntLiteral(v, _) => Some(*v),
            Expr::BoolLiteral(b, _) => Some(u16::from(*b)),
            Expr::Ident(name, _) => self.const_values.get(name).copied(),
            Expr::BinaryOp(lhs, op, rhs, _) => {
                let l = self.eval_const(lhs)?;
                let r = self.eval_const(rhs)?;
                match op {
                    BinOp::Add => Some(l.wrapping_add(r)),
                    BinOp::Sub => Some(l.wrapping_sub(r)),
                    BinOp::Mul => Some(l.wrapping_mul(r)),
                    BinOp::Div if r != 0 => Some(l / r),
                    BinOp::Mod if r != 0 => Some(l % r),
                    BinOp::BitwiseAnd => Some(l & r),
                    BinOp::BitwiseOr => Some(l | r),
                    BinOp::BitwiseXor => Some(l ^ r),
                    BinOp::ShiftLeft => Some(l.wrapping_shl(u32::from(r))),
                    BinOp::ShiftRight => Some(l.wrapping_shr(u32::from(r))),
                    BinOp::Eq => Some(u16::from(l == r)),
                    BinOp::NotEq => Some(u16::from(l != r)),
                    BinOp::Lt => Some(u16::from(l < r)),
                    BinOp::Gt => Some(u16::from(l > r)),
                    BinOp::LtEq => Some(u16::from(l <= r)),
                    BinOp::GtEq => Some(u16::from(l >= r)),
                    _ => None,
                }
            }
            Expr::UnaryOp(op, inner, _) => {
                let v = self.eval_const(inner)?;
                match op {
                    UnaryOp::Negate => Some(v.wrapping_neg()),
                    UnaryOp::BitNot => Some(!v),
                    UnaryOp::Not => Some(u16::from(v == 0)),
                }
            }
            Expr::Cast(inner, _, _) => self.eval_const(inner),
            _ => None,
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
            states: self.state_names,
            start_state: self.start_state,
        }
    }

    fn lower_program(&mut self, program: &Program) {
        // Capture state metadata before lowering
        self.state_names = program.states.iter().map(|s| s.name.clone()).collect();
        program.start_state.clone_into(&mut self.start_state);

        // Register enum variants first so constants that reference
        // them (e.g. `const FIRST: u8 = VariantA`) can resolve.
        for e in &program.enums {
            for (i, (variant, _)) in e.variants.iter().enumerate() {
                self.const_values.insert(variant.clone(), i as u16);
            }
        }

        // Register constants with constant-evaluation. Each const
        // may reference earlier constants.
        for c in &program.constants {
            if let Some(v) = self.eval_const(&c.value) {
                self.const_values.insert(c.name.clone(), v);
            }
        }

        // Lower globals. Initializers can be any constant expression.
        // Struct-literal initializers are expanded into per-field
        // globals so each field gets its own `init_value`; the parent
        // struct itself is still registered (size=0) so any later IR
        // op referencing it by name still resolves. Array-literal
        // initializers are lowered into `init_array` on the parent
        // global — the IR codegen's startup loop emits one LDA/STA
        // per byte into the global's base address.
        for var in &program.globals {
            let var_id = self.get_or_create_var(&var.name);
            let init = var.init.as_ref().and_then(|e| self.eval_const(e));
            let init_array = match &var.init {
                Some(Expr::ArrayLiteral(elems, _)) => elems
                    .iter()
                    .filter_map(|e| self.eval_const(e).map(|v| v as u8))
                    .collect(),
                _ => Vec::new(),
            };
            self.globals.push(IrGlobal {
                var_id,
                name: var.name.clone(),
                size: type_size(&var.var_type),
                init_value: init,
                init_array,
            });
            if let Some(Expr::StructLiteral(_, fields, _)) = &var.init {
                for (fname, fexpr) in fields {
                    let full = format!("{}.{fname}", var.name);
                    let fvid = self.get_or_create_var(&full);
                    let fval = self.eval_const(fexpr);
                    self.globals.push(IrGlobal {
                        var_id: fvid,
                        name: full,
                        size: 1,
                        init_value: fval,
                        init_array: Vec::new(),
                    });
                }
            }
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
        self.current_locals = Vec::new();

        // Register parameters as locals
        for param in &fun.params {
            let var_id = self.get_or_create_var(&param.name);
            self.current_locals.push(IrLocal {
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
            locals: std::mem::take(&mut self.current_locals),
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

        // Lower each scanline handler as a function named
        // `{state}_scanline_{N}`. The IR codegen will generate the MMC3
        // IRQ dispatch wrapper separately.
        for (line, block) in &state.on_scanline {
            let name = format!("{}_scanline_{line}", state.name);
            self.lower_handler(&name, block, state);
        }
    }

    fn lower_handler(&mut self, name: &str, block: &Block, state: &StateDecl) {
        self.next_temp = 0;
        self.current_blocks = Vec::new();
        // Seed `current_locals` with the state's declared locals so any
        // `VarDecl` inside the handler body — tracked by
        // `lower_statement` via `current_locals` — is appended alongside
        // them. Without this, handler-local variables (e.g. a `var i`
        // inside a `while`) would get orphaned: their `VarId` would be
        // created by `get_or_create_var`, but the `IrFunction`'s
        // `locals` list (which the IR codegen uses to allocate RAM
        // addresses) would never see them. The result would be a
        // silent `LoadVar`/`StoreVar` emit-nothing bug that leaves the
        // temp slots uninitialized at runtime.
        self.current_locals = Vec::new();
        for var in &state.locals {
            let var_id = self.get_or_create_var(&var.name);
            self.current_locals.push(IrLocal {
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
            locals: std::mem::take(&mut self.current_locals),
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
                let var_id = self.get_or_create_var(&var.name);
                // Track every local declared inside the current
                // function so the IR codegen can allocate backing
                // storage (e.g. RAM) for it.
                if !self.current_locals.iter().any(|l| l.var_id == var_id) {
                    self.current_locals.push(IrLocal {
                        var_id,
                        name: var.name.clone(),
                        size: type_size(&var.var_type),
                    });
                }
                if let Some(init) = &var.init {
                    // Struct literal initializers expand to per-field
                    // stores on the synthetic field variables.
                    if let Expr::StructLiteral(_, fields, _) = init {
                        for (fname, fexpr) in fields {
                            let full = format!("{}.{fname}", var.name);
                            let fvid = self.get_or_create_var(&full);
                            let val = self.lower_expr(fexpr);
                            self.emit(IrOp::StoreVar(fvid, val));
                        }
                    } else {
                        let val = self.lower_expr(init);
                        self.emit(IrOp::StoreVar(var_id, val));
                    }
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
            Statement::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                // Desugar `for var in start..end { body }` into:
                //     var = start
                //     while var < end { body; var = var + 1 }
                let var_id = self.get_or_create_var(var);
                // The loop variable is implicitly declared by the
                // `for` statement — track it as a local so the IR
                // codegen allocates backing storage. Without this
                // the `StoreVar`/`LoadVar` ops for the counter are
                // silently dropped by `IrCodeGen` (`var_addrs`
                // has no entry), making the counter permanently 0
                // and turning the loop into an infinite one. Same
                // class of bug as handler-local `var` decls before
                // the earlier fix.
                if !self.current_locals.iter().any(|l| l.var_id == var_id) {
                    self.current_locals.push(IrLocal {
                        var_id,
                        name: var.clone(),
                        size: 1,
                    });
                }
                let start_temp = self.lower_expr(start);
                self.emit(IrOp::StoreVar(var_id, start_temp));
                // Precompute the end value once outside the loop
                // header so subsequent iterations don't recompute it.
                // (For a literal, the optimizer collapses this.)
                self.lower_for_body(var_id, end, body);
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
                match name.as_str() {
                    // Built-in `poke(addr, value)` — write a byte to
                    // a compile-time-constant address.
                    "poke" if args.len() == 2 => {
                        if let Some(addr) = self.eval_const(&args[0]) {
                            let val = self.lower_expr(&args[1]);
                            self.emit(IrOp::Poke(addr, val));
                        }
                    }
                    _ => {
                        let arg_temps: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                        self.emit(IrOp::Call(None, name.clone(), arg_temps));
                    }
                }
            }
            Statement::Scroll(x_expr, y_expr, _) => {
                let x = self.lower_expr(x_expr);
                let y = self.lower_expr(y_expr);
                self.emit(IrOp::Scroll(x, y));
            }
            Statement::LoadBackground(_, _) | Statement::SetPalette(_, _) => {
                // TODO: implement in asset pipeline
            }
            Statement::DebugLog(args, _) => {
                let temps: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrOp::DebugLog(temps));
            }
            Statement::DebugAssert(cond, _) => {
                let t = self.lower_expr(cond);
                self.emit(IrOp::DebugAssert(t));
            }
            Statement::InlineAsm(body, _) => {
                self.emit(IrOp::InlineAsm(body.clone()));
            }
            Statement::RawAsm(body, _) => {
                // Raw asm skips `{var}` substitution. We reuse the
                // same IR op variant but mark the body with a magic
                // prefix the codegen can detect — simpler than
                // adding a separate IrOp.
                self.emit(IrOp::InlineAsm(format!("{RAW_ASM_PREFIX}{body}")));
            }
            Statement::Play(_, _) | Statement::StartMusic(_, _) | Statement::StopMusic(_) => {
                // No audio driver yet — these parse but produce no
                // IR.
            }
        }
    }

    fn lower_assign(&mut self, lvalue: &LValue, op: AssignOp, expr: &Expr) {
        // Special case: `var = StructLiteral { ... }` expands to
        // per-field stores against the analyzer-synthesized field
        // variables. This avoids needing struct values as IR temps.
        if let (LValue::Var(name), AssignOp::Assign, Expr::StructLiteral(_, fields, _)) =
            (lvalue, op, expr)
        {
            for (fname, fexpr) in fields {
                let full = format!("{name}.{fname}");
                let field_var = self.get_or_create_var(&full);
                let val = self.lower_expr(fexpr);
                self.emit(IrOp::StoreVar(field_var, val));
            }
            return;
        }

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
                            AssignOp::ShiftLeftAssign => IrOp::ShiftLeft(result, current, 1),
                            AssignOp::ShiftRightAssign => IrOp::ShiftRight(result, current, 1),
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
                if op == AssignOp::Assign {
                    self.emit(IrOp::ArrayStore(var_id, idx, val));
                } else {
                    let current = self.fresh_temp();
                    self.emit(IrOp::ArrayLoad(current, var_id, idx));
                    let result = self.fresh_temp();
                    let ir_op = match op {
                        AssignOp::PlusAssign => IrOp::Add(result, current, val),
                        AssignOp::MinusAssign => IrOp::Sub(result, current, val),
                        AssignOp::AmpAssign => IrOp::And(result, current, val),
                        AssignOp::PipeAssign => IrOp::Or(result, current, val),
                        AssignOp::CaretAssign => IrOp::Xor(result, current, val),
                        AssignOp::ShiftLeftAssign => IrOp::ShiftLeft(result, current, 1),
                        AssignOp::ShiftRightAssign => IrOp::ShiftRight(result, current, 1),
                        AssignOp::Assign => unreachable!(),
                    };
                    self.emit(ir_op);
                    self.emit(IrOp::ArrayStore(var_id, idx, result));
                }
            }
            LValue::Field(name, field) => {
                // The analyzer synthesizes a variable named
                // `"struct.field"` for each struct field, so we can
                // treat field assignment as a regular variable
                // assignment to that synthetic name.
                let full_name = format!("{name}.{field}");
                let var_id = self.get_or_create_var(&full_name);
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
                            AssignOp::ShiftLeftAssign => IrOp::ShiftLeft(result, current, 1),
                            AssignOp::ShiftRightAssign => IrOp::ShiftRight(result, current, 1),
                            AssignOp::Assign => unreachable!(),
                        };
                        self.emit(ir_op);
                        self.emit(IrOp::StoreVar(var_id, result));
                    }
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

    /// Lower the loop body for a `for var in start..end { body }`.
    /// Assumes `var` has already been initialized to the start
    /// value. Emits the condition `var < end` each iteration and
    /// increments `var` at the continue edge.
    fn lower_for_body(&mut self, var_id: VarId, end: &Expr, body: &Block) {
        let cond_label = self.fresh_label("for_cond");
        let body_label = self.fresh_label("for_body");
        let end_label = self.fresh_label("for_end");

        self.end_block(IrTerminator::Jump(cond_label.clone()));

        // Condition: var < end
        self.start_block(&cond_label);
        let var_temp = self.fresh_temp();
        self.emit(IrOp::LoadVar(var_temp, var_id));
        let end_temp = self.lower_expr(end);
        let cmp_temp = self.fresh_temp();
        self.emit(IrOp::CmpLt(cmp_temp, var_temp, end_temp));
        self.end_block(IrTerminator::Branch(
            cmp_temp,
            body_label.clone(),
            end_label.clone(),
        ));

        // Body + increment.
        let step_label = self.fresh_label("for_step");
        self.loop_stack.push(LoopContext {
            continue_label: step_label.clone(),
            break_label: end_label.clone(),
        });
        self.start_block(&body_label);
        self.lower_block(body);
        self.end_block(IrTerminator::Jump(step_label.clone()));
        self.loop_stack.pop();

        // Step: var = var + 1
        self.start_block(&step_label);
        let cur = self.fresh_temp();
        self.emit(IrOp::LoadVar(cur, var_id));
        let one = self.fresh_temp();
        self.emit(IrOp::LoadImm(one, 1));
        let next = self.fresh_temp();
        self.emit(IrOp::Add(next, cur, one));
        self.emit(IrOp::StoreVar(var_id, next));
        self.end_block(IrTerminator::Jump(cond_label));

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
            Expr::FieldAccess(name, field, _) => {
                // Field access lowers to a plain load of the
                // synthetic `"struct.field"` variable produced by the
                // analyzer.
                let full_name = format!("{name}.{field}");
                let var_id = self.get_or_create_var(&full_name);
                let t = self.fresh_temp();
                self.emit(IrOp::LoadVar(t, var_id));
                t
            }
            Expr::BinaryOp(left, op, right, _) => self.lower_binop(left, *op, right),
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
                // Built-in `peek(addr)` reads a byte from a fixed
                // absolute address at compile time.
                if name == "peek" && args.len() == 1 {
                    if let Some(addr) = self.eval_const(&args[0]) {
                        let t = self.fresh_temp();
                        self.emit(IrOp::Peek(t, addr));
                        return t;
                    }
                }
                let arg_temps: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                let t = self.fresh_temp();
                self.emit(IrOp::Call(Some(t), name.clone(), arg_temps));
                t
            }
            Expr::ButtonRead(player, button, _) => {
                // Button reads: read the input byte, mask with the button bit.
                // Player 1 reads from $01, player 2 reads from $08.
                let player_index = match player {
                    Some(Player::P2) => 1u8,
                    _ => 0u8,
                };
                let input = self.fresh_temp();
                self.emit(IrOp::ReadInput(input, player_index));
                let mask = button_mask(button);
                let mask_temp = self.fresh_temp();
                self.emit(IrOp::LoadImm(mask_temp, mask));
                let t = self.fresh_temp();
                self.emit(IrOp::And(t, input, mask_temp));
                t
            }
            Expr::ArrayLiteral(_, _) => {
                // Array literals are handled during initialization, not as general expressions
                let t = self.fresh_temp();
                self.emit(IrOp::LoadImm(t, 0));
                t
            }
            Expr::StructLiteral(_, _, _) => {
                // Struct literals are only supported as the right
                // hand side of a plain assignment (see lower_assign).
                // Falling through here means the literal was used in
                // an expression context the lowering can't handle;
                // emit zero so the build still produces a ROM.
                let t = self.fresh_temp();
                self.emit(IrOp::LoadImm(t, 0));
                t
            }
            Expr::Cast(inner, _, _) => {
                // For now, just evaluate the inner expression (truncation/extension is a no-op on 8-bit)
                self.lower_expr(inner)
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

    /// Emit an IR "move" from `src` to `dest`: `dest = src | 0`.
    /// Used to merge values from different control-flow paths.
    fn emit_move(&mut self, dest: IrTemp, src: IrTemp) {
        let zero = self.fresh_temp();
        self.emit(IrOp::LoadImm(zero, 0));
        self.emit(IrOp::Or(dest, src, zero));
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
        self.emit_move(result, r);
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
        self.emit_move(result, r);
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
        // Struct sizes are resolved in the analyzer. IR lowering only
        // sees struct types on `var` declarations, which are skipped
        // below via the analyzer's synthetic field allocations.
        NesType::Struct(_) => 0,
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
