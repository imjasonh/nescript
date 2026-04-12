#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::errors::{Diagnostic, ErrorCode, Label, Level};
use crate::lexer::Span;
use crate::parser::ast::*;

/// Symbol information stored in the scope.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub sym_type: NesType,
    pub is_const: bool,
    pub span: Span,
}

/// Memory assignment for a variable.
#[derive(Debug, Clone)]
pub struct VarAllocation {
    pub name: String,
    pub address: u16,
    pub size: u16,
}

/// Result of semantic analysis.
pub struct AnalysisResult {
    pub symbols: HashMap<String, Symbol>,
    pub var_allocations: Vec<VarAllocation>,
    pub diagnostics: Vec<Diagnostic>,
    pub call_graph: HashMap<String, Vec<String>>,
    pub max_depths: HashMap<String, u32>,
}

/// Default call stack depth limit for the NES runtime.
const DEFAULT_STACK_DEPTH: u32 = 8;

/// Analyze a parsed program for semantic errors.
pub fn analyze(program: &Program) -> AnalysisResult {
    let mut analyzer = Analyzer {
        symbols: HashMap::new(),
        var_allocations: Vec::new(),
        diagnostics: Vec::new(),
        next_ram_addr: 0x0300, // $0300 is first usable RAM after OAM buffer
        next_zp_addr: 0x10,    // $10 is first usable zero-page after reserved area
        call_graph: HashMap::new(),
        max_depths: HashMap::new(),
        stack_depth_limit: DEFAULT_STACK_DEPTH,
        in_loop: false,
        used_vars: HashSet::new(),
    };
    analyzer.analyze_program(program);

    AnalysisResult {
        symbols: analyzer.symbols,
        var_allocations: analyzer.var_allocations,
        diagnostics: analyzer.diagnostics,
        call_graph: analyzer.call_graph,
        max_depths: analyzer.max_depths,
    }
}

struct Analyzer {
    symbols: HashMap<String, Symbol>,
    var_allocations: Vec<VarAllocation>,
    diagnostics: Vec<Diagnostic>,
    next_ram_addr: u16,
    next_zp_addr: u8,
    call_graph: HashMap<String, Vec<String>>,
    max_depths: HashMap<String, u32>,
    stack_depth_limit: u32,
    in_loop: bool,
    /// Names of variables that have been read somewhere in the program.
    /// Used for the W0103 unused-variable warning.
    used_vars: HashSet<String>,
}

impl Analyzer {
    fn analyze_program(&mut self, program: &Program) {
        // Register constants
        for c in &program.constants {
            self.register_const(c);
        }

        // Register and allocate globals
        for var in &program.globals {
            self.register_var(var);
        }

        // Register functions as symbols
        for fun in &program.functions {
            self.register_fun(fun);
        }

        // Register state-local variables
        for state in &program.states {
            for var in &state.locals {
                self.register_var(var);
            }
        }

        // Validate state references
        let state_names: Vec<&str> = program.states.iter().map(|s| s.name.as_str()).collect();

        // Check start state exists
        if !state_names.contains(&program.start_state.as_str()) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0404,
                format!("start state '{}' is not defined", program.start_state),
                program.span,
            ));
        }

        // Type-check all state bodies
        for state in &program.states {
            if let Some(block) = &state.on_enter {
                self.check_block(block, &state_names);
            }
            if let Some(block) = &state.on_exit {
                self.check_block(block, &state_names);
            }
            if let Some(block) = &state.on_frame {
                self.check_block(block, &state_names);
            }
        }

        // Type-check function bodies. Parameters are registered as
        // symbols for the duration of the body check so that identifier
        // references (and the W0103 used-variable tracker) can resolve
        // them. They are unregistered afterwards to avoid leaking into
        // the global scope. Parameters are also pre-marked as "used" so
        // we do not emit W0103 for unused function arguments (which are
        // a common and deliberate pattern).
        for fun in &program.functions {
            let mut added_params = Vec::new();
            for param in &fun.params {
                if !self.symbols.contains_key(&param.name) {
                    self.symbols.insert(
                        param.name.clone(),
                        Symbol {
                            name: param.name.clone(),
                            sym_type: param.param_type.clone(),
                            is_const: false,
                            span: fun.span,
                        },
                    );
                    added_params.push(param.name.clone());
                }
                self.mark_var_used(&param.name);
            }
            self.check_block(&fun.body, &state_names);
            for name in &added_params {
                self.symbols.remove(name);
            }
        }

        // Build call graph
        self.build_call_graph(program);

        // Detect recursion
        let recursive_fns = detect_recursion(&self.call_graph);
        for name in &recursive_fns {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0402,
                format!("recursion detected in function '{name}'"),
                program.span,
            ));
        }

        // Compute max call depths from entry points (state handlers)
        self.compute_max_depths(program);

        // Check for unused global variables (W0103). Variables whose names
        // start with '_' are exempt by convention. State-local variables are
        // left out for now to avoid noise during early development.
        for var in &program.globals {
            if var.name.starts_with('_') {
                continue;
            }
            if !self.used_vars.contains(&var.name) {
                self.diagnostics.push(Diagnostic {
                    level: Level::Warning,
                    code: ErrorCode::W0103,
                    message: format!("unused variable '{}'", var.name),
                    span: var.span,
                    labels: Vec::<Label>::new(),
                    help: Some(
                        "prefix with '_' to silence this warning, or remove the declaration".into(),
                    ),
                    note: None,
                });
            }
        }

        // Check for unreachable states (W0104).
        self.check_unreachable_states(program);
    }

    /// Mark a variable name as having been read somewhere in the program.
    fn mark_var_used(&mut self, name: &str) {
        self.used_vars.insert(name.to_string());
    }

    /// Recursively walk an expression tree and mark every identifier that
    /// appears as an `Expr::Ident` (or as an `Expr::ArrayIndex` base) as
    /// "read". Used by the W0103 unused-variable analysis. Also emits
    /// E0502 for any identifier that is not defined in the symbol table.
    fn walk_expr_reads(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name, span) => {
                if self.symbols.contains_key(name) {
                    self.mark_var_used(name);
                } else {
                    self.emit_undefined_var(name, *span);
                }
            }
            Expr::ArrayIndex(name, idx, span) => {
                // Array base is a read; index may contain more reads.
                if self.symbols.contains_key(name) {
                    self.mark_var_used(name);
                } else {
                    self.emit_undefined_var(name, *span);
                }
                self.walk_expr_reads(idx);
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                self.walk_expr_reads(lhs);
                self.walk_expr_reads(rhs);
            }
            Expr::UnaryOp(_, inner, _) | Expr::Cast(inner, _, _) => {
                self.walk_expr_reads(inner);
            }
            Expr::Call(_, args, _) => {
                // Function name is validated separately via E0503; here we
                // just recurse into argument expressions so their reads
                // get tracked (and undefined-var errors surface).
                for arg in args {
                    self.walk_expr_reads(arg);
                }
            }
            Expr::ArrayLiteral(elems, _) => {
                for e in elems {
                    self.walk_expr_reads(e);
                }
            }
            Expr::IntLiteral(_, _) | Expr::BoolLiteral(_, _) | Expr::ButtonRead(_, _, _) => {}
        }
    }

    /// Suggest a similarly-named symbol for undefined-variable errors.
    /// Uses a simple heuristic: same first character and similar length.
    fn suggest_var_name(&self, unknown: &str) -> Option<String> {
        let first = unknown.chars().next()?;
        self.symbols
            .keys()
            .filter(|name| {
                name.starts_with(first)
                    && name.len().abs_diff(unknown.len()) <= 2
                    && name.as_str() != unknown
            })
            .min_by_key(|name| name.len().abs_diff(unknown.len()))
            .cloned()
    }

    /// Emit E0502 for an undefined variable reference, with a "did you mean"
    /// suggestion if a similar symbol exists.
    fn emit_undefined_var(&mut self, name: &str, span: Span) {
        let mut diag = Diagnostic::error(
            ErrorCode::E0502,
            format!("undefined variable '{name}'"),
            span,
        );
        if let Some(suggestion) = self.suggest_var_name(name) {
            diag = diag.with_help(format!("did you mean '{suggestion}'?"));
        }
        self.diagnostics.push(diag);
    }

    /// Reachability analysis for states. Performs a BFS from the start state
    /// through every transition in state handlers and emits W0104 for any
    /// state that is never reached.
    fn check_unreachable_states(&mut self, program: &Program) {
        let mut reachable: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = vec![program.start_state.clone()];

        while let Some(state_name) = queue.pop() {
            if !reachable.insert(state_name.clone()) {
                continue;
            }
            if let Some(state) = program.states.iter().find(|s| s.name == state_name) {
                collect_transitions_from_state(state, &mut queue);
            }
        }

        for state in &program.states {
            if !reachable.contains(&state.name) {
                self.diagnostics.push(Diagnostic {
                    level: Level::Warning,
                    code: ErrorCode::W0104,
                    message: format!("state '{}' is unreachable from start state", state.name),
                    span: state.span,
                    labels: Vec::<Label>::new(),
                    help: Some(
                        "add a 'transition' to this state from a reachable state, or remove it"
                            .into(),
                    ),
                    note: None,
                });
            }
        }
    }

    fn register_const(&mut self, c: &ConstDecl) {
        if self.symbols.contains_key(&c.name) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0501,
                format!("duplicate declaration of '{}'", c.name),
                c.span,
            ));
            return;
        }
        self.symbols.insert(
            c.name.clone(),
            Symbol {
                name: c.name.clone(),
                sym_type: c.const_type.clone(),
                is_const: true,
                span: c.span,
            },
        );
    }

    fn register_var(&mut self, var: &VarDecl) {
        if self.symbols.contains_key(&var.name) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0501,
                format!("duplicate declaration of '{}'", var.name),
                var.span,
            ));
            return;
        }

        let size = type_size(&var.var_type);
        let address = self.allocate_ram(size);

        self.symbols.insert(
            var.name.clone(),
            Symbol {
                name: var.name.clone(),
                sym_type: var.var_type.clone(),
                is_const: false,
                span: var.span,
            },
        );

        self.var_allocations.push(VarAllocation {
            name: var.name.clone(),
            address,
            size,
        });
    }

    fn register_fun(&mut self, fun: &FunDecl) {
        if self.symbols.contains_key(&fun.name) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0501,
                format!("duplicate declaration of '{}'", fun.name),
                fun.span,
            ));
            return;
        }
        let sym_type = fun.return_type.clone().unwrap_or(NesType::U8);
        self.symbols.insert(
            fun.name.clone(),
            Symbol {
                name: fun.name.clone(),
                sym_type,
                is_const: false,
                span: fun.span,
            },
        );
    }

    fn allocate_ram(&mut self, size: u16) -> u16 {
        // For M1: simple linear allocator using zero-page for u8 vars
        if size == 1 && self.next_zp_addr < 0xFF {
            let addr = u16::from(self.next_zp_addr);
            self.next_zp_addr = self.next_zp_addr.wrapping_add(1);
            addr
        } else {
            let addr = self.next_ram_addr;
            self.next_ram_addr += size;
            addr
        }
    }

    fn build_call_graph(&mut self, program: &Program) {
        // Record calls from each function body
        for fun in &program.functions {
            let callees = collect_calls(&fun.body);
            self.call_graph.insert(fun.name.clone(), callees);
        }

        // Record calls from each state handler
        for state in &program.states {
            if let Some(block) = &state.on_enter {
                let key = format!("{}::enter", state.name);
                let callees = collect_calls(block);
                self.call_graph.insert(key, callees);
            }
            if let Some(block) = &state.on_exit {
                let key = format!("{}::exit", state.name);
                let callees = collect_calls(block);
                self.call_graph.insert(key, callees);
            }
            if let Some(block) = &state.on_frame {
                let key = format!("{}::frame", state.name);
                let callees = collect_calls(block);
                self.call_graph.insert(key, callees);
            }
        }
    }

    fn compute_max_depths(&mut self, program: &Program) {
        let mut cache = HashMap::new();

        // Entry points are state handlers
        for state in &program.states {
            let handler_keys: Vec<String> = [
                state
                    .on_enter
                    .as_ref()
                    .map(|_| format!("{}::enter", state.name)),
                state
                    .on_exit
                    .as_ref()
                    .map(|_| format!("{}::exit", state.name)),
                state
                    .on_frame
                    .as_ref()
                    .map(|_| format!("{}::frame", state.name)),
            ]
            .into_iter()
            .flatten()
            .collect();

            for key in handler_keys {
                let mut visited = HashSet::new();
                let depth = compute_depth(&key, &self.call_graph, &mut visited, &mut cache);
                self.max_depths.insert(key.clone(), depth);

                if depth > self.stack_depth_limit {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0401,
                        format!(
                            "call depth {depth} in handler '{key}' exceeds stack limit {}",
                            self.stack_depth_limit
                        ),
                        program.span,
                    ));
                }
            }
        }
    }

    fn check_block(&mut self, block: &Block, state_names: &[&str]) {
        for stmt in &block.statements {
            self.check_statement(stmt, state_names);
        }
    }

    fn check_statement(&mut self, stmt: &Statement, state_names: &[&str]) {
        match stmt {
            Statement::VarDecl(var) => {
                self.register_var(var);
                if let Some(init) = &var.init {
                    self.walk_expr_reads(init);
                    self.check_expr_type(init, &var.var_type);
                }
            }
            Statement::Assign(lvalue, _, expr, span) => {
                // Check if trying to assign to a constant
                match lvalue {
                    LValue::Var(name) => {
                        if let Some(sym) = self.symbols.get(name) {
                            if sym.is_const {
                                self.diagnostics.push(Diagnostic::error(
                                    ErrorCode::E0203,
                                    format!("cannot assign to constant '{name}'"),
                                    *span,
                                ));
                            }
                        }
                    }
                    LValue::ArrayIndex(name, idx) => {
                        if let Some(sym) = self.symbols.get(name) {
                            if sym.is_const {
                                self.diagnostics.push(Diagnostic::error(
                                    ErrorCode::E0203,
                                    format!("cannot assign to constant '{name}'"),
                                    *span,
                                ));
                            }
                        }
                        // Indexing an array counts as a read of the array,
                        // and the index expression itself may contain reads.
                        self.mark_var_used(name);
                        self.walk_expr_reads(idx);
                    }
                }
                self.walk_expr_reads(expr);
                let ltype = self.lvalue_type(lvalue, *span);
                if let Some(lt) = ltype {
                    self.check_expr_type(expr, &lt);
                }
            }
            Statement::If(cond, then_block, else_ifs, else_block, _) => {
                self.walk_expr_reads(cond);
                self.check_expr_type(cond, &NesType::Bool);
                self.check_block(then_block, state_names);
                for (cond, block) in else_ifs {
                    self.walk_expr_reads(cond);
                    self.check_expr_type(cond, &NesType::Bool);
                    self.check_block(block, state_names);
                }
                if let Some(block) = else_block {
                    self.check_block(block, state_names);
                }
            }
            Statement::While(cond, body, _) => {
                self.walk_expr_reads(cond);
                self.check_expr_type(cond, &NesType::Bool);
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_block(body, state_names);
                self.in_loop = was_in_loop;
            }
            Statement::Loop(body, _) => {
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_block(body, state_names);
                self.in_loop = was_in_loop;
            }
            Statement::Transition(name, span) => {
                if !state_names.contains(&name.as_str()) {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0404,
                        format!("transition to undefined state '{name}'"),
                        *span,
                    ));
                }
            }
            Statement::Draw(draw) => {
                self.walk_expr_reads(&draw.x);
                self.walk_expr_reads(&draw.y);
                self.check_expr_type(&draw.x, &NesType::U8);
                self.check_expr_type(&draw.y, &NesType::U8);
                if let Some(frame) = &draw.frame {
                    self.walk_expr_reads(frame);
                    self.check_expr_type(frame, &NesType::U8);
                }
            }
            Statement::Return(Some(expr), _) => {
                // For M1, just validate the expression without checking return type
                self.walk_expr_reads(expr);
                let _ = self.infer_type(expr);
            }
            Statement::Call(name, args, span) => {
                if !self.symbols.contains_key(name) {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0503,
                        format!("undefined function '{name}'"),
                        *span,
                    ));
                }
                for arg in args {
                    self.walk_expr_reads(arg);
                }
            }
            Statement::Scroll(x, y, _) => {
                self.walk_expr_reads(x);
                self.walk_expr_reads(y);
                self.check_expr_type(x, &NesType::U8);
                self.check_expr_type(y, &NesType::U8);
            }
            Statement::Break(span) => {
                if !self.in_loop {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        "break outside of loop",
                        *span,
                    ));
                }
            }
            Statement::Continue(span) => {
                if !self.in_loop {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        "continue outside of loop",
                        *span,
                    ));
                }
            }
            Statement::WaitFrame(_)
            | Statement::Return(None, _)
            | Statement::LoadBackground(_, _)
            | Statement::SetPalette(_, _) => {}
            Statement::DebugLog(args, _) => {
                for arg in args {
                    self.walk_expr_reads(arg);
                }
            }
            Statement::DebugAssert(cond, _) => {
                self.walk_expr_reads(cond);
                self.check_expr_type(cond, &NesType::Bool);
            }
        }
    }

    fn lvalue_type(&self, lvalue: &LValue, _span: Span) -> Option<NesType> {
        match lvalue {
            LValue::Var(name) => self.symbols.get(name).map(|s| s.sym_type.clone()),
            LValue::ArrayIndex(name, _) => {
                self.symbols.get(name).and_then(|sym| match &sym.sym_type {
                    NesType::Array(elem, _) => Some(elem.as_ref().clone()),
                    _ => None,
                })
            }
        }
    }

    fn check_expr_type(&mut self, expr: &Expr, expected: &NesType) {
        let actual = self.infer_type(expr);
        if let Some(actual) = actual {
            // Allow numeric comparisons to produce bool
            if *expected == NesType::Bool && actual == NesType::Bool {
                return;
            }
            // For M1: be lenient about integer types in conditions
            // button reads produce bool
            if *expected == NesType::Bool {
                match expr {
                    Expr::ButtonRead(..)
                    | Expr::BinaryOp(
                        _,
                        BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Lt
                        | BinOp::Gt
                        | BinOp::LtEq
                        | BinOp::GtEq,
                        _,
                        _,
                    )
                    | Expr::UnaryOp(UnaryOp::Not, _, _)
                    | Expr::BinaryOp(_, BinOp::And | BinOp::Or, _, _) => return,
                    _ => {}
                }
            }
            if actual != *expected {
                // Allow implicit u8/i8/u16 in assignments for M1 simplicity
                if is_integer_type(&actual) && is_integer_type(expected) {
                    return;
                }
                self.diagnostics.push(
                    Diagnostic::error(
                        ErrorCode::E0201,
                        format!("type mismatch: expected {expected}, found {actual}"),
                        expr.span(),
                    )
                    .with_help(format!("use 'as {expected}' for explicit conversion")),
                );
            }
        }
    }

    fn infer_type(&self, expr: &Expr) -> Option<NesType> {
        match expr {
            Expr::IntLiteral(v, _) => {
                if *v <= 255 {
                    Some(NesType::U8)
                } else {
                    Some(NesType::U16)
                }
            }
            Expr::BoolLiteral(_, _) => Some(NesType::Bool),
            Expr::Ident(name, _) => self.symbols.get(name).map(|s| s.sym_type.clone()),
            Expr::ButtonRead(_, _, _) => Some(NesType::Bool),
            Expr::BinaryOp(_, op, _, _) => match op {
                BinOp::Eq
                | BinOp::NotEq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::LtEq
                | BinOp::GtEq
                | BinOp::And
                | BinOp::Or => Some(NesType::Bool),
                _ => Some(NesType::U8), // Simplified for M1
            },
            Expr::UnaryOp(UnaryOp::Not, _, _) => Some(NesType::Bool),
            Expr::UnaryOp(_, _, _) => Some(NesType::U8),
            Expr::Call(_, _, _) => Some(NesType::U8), // Simplified for M1
            Expr::ArrayIndex(name, _, _) => {
                self.symbols.get(name).and_then(|s| match &s.sym_type {
                    NesType::Array(elem, _) => Some(elem.as_ref().clone()),
                    _ => None,
                })
            }
            Expr::ArrayLiteral(_, _) => Some(NesType::U8), // element type inferred from context
            Expr::Cast(_, target, _) => Some(target.clone()),
        }
    }
}

/// Collect every state name mentioned in a transition statement inside the
/// given state's handlers and append them to `queue`. Used by the W0104
/// unreachable-state check.
fn collect_transitions_from_state(state: &StateDecl, queue: &mut Vec<String>) {
    if let Some(block) = &state.on_enter {
        collect_transitions_block(block, queue);
    }
    if let Some(block) = &state.on_exit {
        collect_transitions_block(block, queue);
    }
    if let Some(block) = &state.on_frame {
        collect_transitions_block(block, queue);
    }
    for (_, block) in &state.on_scanline {
        collect_transitions_block(block, queue);
    }
}

fn collect_transitions_block(block: &Block, queue: &mut Vec<String>) {
    for stmt in &block.statements {
        collect_transitions_stmt(stmt, queue);
    }
}

fn collect_transitions_stmt(stmt: &Statement, queue: &mut Vec<String>) {
    match stmt {
        Statement::Transition(name, _) => queue.push(name.clone()),
        Statement::If(_, then_b, elifs, else_b, _) => {
            collect_transitions_block(then_b, queue);
            for (_, b) in elifs {
                collect_transitions_block(b, queue);
            }
            if let Some(b) = else_b {
                collect_transitions_block(b, queue);
            }
        }
        Statement::While(_, body, _) | Statement::Loop(body, _) => {
            collect_transitions_block(body, queue);
        }
        _ => {}
    }
}

/// Collect all function/call names from a block.
fn collect_calls(block: &Block) -> Vec<String> {
    let mut calls = Vec::new();
    for stmt in &block.statements {
        collect_calls_stmt(stmt, &mut calls);
    }
    calls
}

fn collect_calls_stmt(stmt: &Statement, calls: &mut Vec<String>) {
    match stmt {
        Statement::Call(name, args, _) => {
            calls.push(name.clone());
            for arg in args {
                collect_calls_expr(arg, calls);
            }
        }
        Statement::If(cond, then_b, elifs, else_b, _) => {
            collect_calls_expr(cond, calls);
            collect_calls_block(then_b, calls);
            for (c, b) in elifs {
                collect_calls_expr(c, calls);
                collect_calls_block(b, calls);
            }
            if let Some(b) = else_b {
                collect_calls_block(b, calls);
            }
        }
        Statement::While(cond, body, _) => {
            collect_calls_expr(cond, calls);
            collect_calls_block(body, calls);
        }
        Statement::Loop(body, _) => {
            collect_calls_block(body, calls);
        }
        Statement::Assign(_, _, expr, _) => {
            collect_calls_expr(expr, calls);
        }
        Statement::VarDecl(var) => {
            if let Some(init) = &var.init {
                collect_calls_expr(init, calls);
            }
        }
        Statement::Return(Some(expr), _) => {
            collect_calls_expr(expr, calls);
        }
        Statement::Draw(draw) => {
            collect_calls_expr(&draw.x, calls);
            collect_calls_expr(&draw.y, calls);
            if let Some(f) = &draw.frame {
                collect_calls_expr(f, calls);
            }
        }
        Statement::Scroll(x, y, _) => {
            collect_calls_expr(x, calls);
            collect_calls_expr(y, calls);
        }
        Statement::DebugLog(args, _) => {
            for arg in args {
                collect_calls_expr(arg, calls);
            }
        }
        Statement::DebugAssert(cond, _) => {
            collect_calls_expr(cond, calls);
        }
        Statement::Return(None, _)
        | Statement::Transition(_, _)
        | Statement::WaitFrame(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::LoadBackground(_, _)
        | Statement::SetPalette(_, _) => {}
    }
}

fn collect_calls_block(block: &Block, calls: &mut Vec<String>) {
    for stmt in &block.statements {
        collect_calls_stmt(stmt, calls);
    }
}

fn collect_calls_expr(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Call(name, args, _) => {
            calls.push(name.clone());
            for arg in args {
                collect_calls_expr(arg, calls);
            }
        }
        Expr::BinaryOp(lhs, _, rhs, _) => {
            collect_calls_expr(lhs, calls);
            collect_calls_expr(rhs, calls);
        }
        Expr::UnaryOp(_, inner, _) => {
            collect_calls_expr(inner, calls);
        }
        Expr::ArrayIndex(_, idx, _) => {
            collect_calls_expr(idx, calls);
        }
        Expr::ArrayLiteral(elems, _) => {
            for e in elems {
                collect_calls_expr(e, calls);
            }
        }
        Expr::Cast(inner, _, _) => {
            collect_calls_expr(inner, calls);
        }
        Expr::IntLiteral(_, _)
        | Expr::BoolLiteral(_, _)
        | Expr::Ident(_, _)
        | Expr::ButtonRead(_, _, _) => {}
    }
}

/// Detect cycles in the call graph using DFS. Returns the names of all
/// functions that participate in a cycle (direct or mutual recursion).
fn detect_recursion(graph: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut recursive = Vec::new();
    let mut visited = HashSet::new();
    let mut on_stack = HashSet::new();

    for node in graph.keys() {
        if !visited.contains(node) {
            detect_recursion_dfs(node, graph, &mut visited, &mut on_stack, &mut recursive);
        }
    }

    recursive.sort();
    recursive.dedup();
    recursive
}

fn detect_recursion_dfs(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    on_stack: &mut HashSet<String>,
    recursive: &mut Vec<String>,
) {
    visited.insert(node.to_string());
    on_stack.insert(node.to_string());

    if let Some(callees) = graph.get(node) {
        for callee in callees {
            if on_stack.contains(callee) {
                // Found a cycle — mark the callee (the one we recursed back to)
                recursive.push(callee.clone());
            } else if !visited.contains(callee) {
                detect_recursion_dfs(callee, graph, visited, on_stack, recursive);
            }
        }
    }

    on_stack.remove(node);
}

/// Compute the maximum call depth starting from a given node in the call graph.
/// Returns `None` if a cycle is encountered (handled separately by recursion detection).
fn compute_depth(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    cache: &mut HashMap<String, u32>,
) -> u32 {
    if let Some(&depth) = cache.get(node) {
        return depth;
    }
    if visited.contains(node) {
        // Cycle — return 0 to avoid infinite recursion; the cycle itself
        // is flagged by detect_recursion.
        return 0;
    }
    visited.insert(node.to_string());
    let mut max_child: u32 = 0;
    if let Some(callees) = graph.get(node) {
        for callee in callees {
            let child = compute_depth(callee, graph, visited, cache);
            max_child = max_child.max(child);
        }
    }
    visited.remove(node);
    let depth = if graph.get(node).is_none_or(Vec::is_empty) {
        0
    } else {
        1 + max_child
    };
    cache.insert(node.to_string(), depth);
    depth
}

fn type_size(t: &NesType) -> u16 {
    match t {
        NesType::U8 | NesType::I8 | NesType::Bool => 1,
        NesType::U16 => 2,
        NesType::Array(elem, count) => type_size(elem) * count,
    }
}

fn is_integer_type(t: &NesType) -> bool {
    matches!(t, NesType::U8 | NesType::I8 | NesType::U16)
}
