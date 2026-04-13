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

/// Upper bound (exclusive) for user-variable zero-page allocation.
/// Addresses `$80-$FF` are reserved for IR codegen temp slots, so user
/// globals must fit into `$10-$7F`.
const ZP_USER_CAP: u8 = 0x80;

/// Exclusive upper bound of usable RAM. The NES has 2 KB of internal
/// RAM at `$0000-$07FF`; the allocator uses up through `$07FF`.
const RAM_END: u16 = 0x0800;

/// Analyze a parsed program for semantic errors.
pub fn analyze(program: &Program) -> AnalysisResult {
    // Pre-collect declared and builtin-matchable sfx/music names so
    // the statement walker can check `play` / `start_music` targets.
    let mut sfx_names = HashSet::new();
    for decl in &program.sfx {
        sfx_names.insert(decl.name.clone());
    }
    let mut music_names = HashSet::new();
    for decl in &program.music {
        music_names.insert(decl.name.clone());
    }
    let mut analyzer = Analyzer {
        symbols: HashMap::new(),
        var_allocations: Vec::new(),
        diagnostics: Vec::new(),
        sfx_names,
        music_names,
        next_ram_addr: 0x0300, // $0300 is first usable RAM after OAM buffer
        next_zp_addr: 0x10,    // $10 is first usable zero-page after reserved area
        call_graph: HashMap::new(),
        max_depths: HashMap::new(),
        stack_depth_limit: DEFAULT_STACK_DEPTH,
        in_loop: false,
        used_vars: HashSet::new(),
        function_signatures: HashMap::new(),
        current_return_type: None,
        in_function_body: false,
        struct_layouts: HashMap::new(),
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
    /// Set of sfx names declared in the program (user-provided
    /// `sfx Name { ... }` blocks). Used to validate `play Name`
    /// targets. If a name isn't in here and isn't a known builtin,
    /// the analyzer emits E0505.
    sfx_names: HashSet<String>,
    /// Set of music names declared in the program.
    music_names: HashSet<String>,
    next_ram_addr: u16,
    next_zp_addr: u8,
    call_graph: HashMap<String, Vec<String>>,
    max_depths: HashMap<String, u32>,
    stack_depth_limit: u32,
    in_loop: bool,
    /// Names of variables that have been read somewhere in the program.
    /// Used for the W0103 unused-variable warning.
    used_vars: HashSet<String>,
    /// Function name to parameter types (in order). Used to validate
    /// call arity and argument types.
    function_signatures: HashMap<String, Vec<NesType>>,
    /// Return type of the function currently being analyzed, or None
    /// when the function has no declared return type. Only meaningful
    /// when `in_function_body` is true.
    current_return_type: Option<NesType>,
    /// True while analyzing a function body (as opposed to a state
    /// handler's `on_enter` / `on_exit` / `on_frame` block). Used to
    /// distinguish "void function" from "state handler" when checking
    /// `return value` statements.
    in_function_body: bool,
    /// Struct name to layout. Each field has an offset in bytes from
    /// the base address of the struct.
    struct_layouts: HashMap<String, StructLayout>,
}

/// Layout info for a struct type.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub size: u16,
    pub fields: Vec<(String, NesType, u16)>, // (name, type, offset)
}

impl Analyzer {
    fn analyze_program(&mut self, program: &Program) {
        // Register struct layouts first so later declarations can
        // reference them (for variable sizing, etc.).
        for s in &program.structs {
            self.register_struct(s);
        }

        // Register constants
        for c in &program.constants {
            self.register_const(c);
        }

        // Register enum variants as constants with values 0, 1, 2, ...
        for e in &program.enums {
            self.register_enum(e);
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
            // `on scanline(N)` is only valid with mappers that have a
            // scanline-counting IRQ source (currently only MMC3).
            if !state.on_scanline.is_empty() && program.game.mapper != Mapper::MMC3 {
                self.diagnostics.push(Diagnostic::error(
                    ErrorCode::E0203,
                    "`on scanline` requires the MMC3 mapper",
                    state.span,
                ));
            }
            for (_, block) in &state.on_scanline {
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
            self.current_return_type.clone_from(&fun.return_type);
            self.in_function_body = true;
            self.check_block(&fun.body, &state_names);
            self.current_return_type = None;
            self.in_function_body = false;
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

        // Check for unused variables (W0103). Variables whose names
        // start with '_' are exempt by convention. Both globals and
        // state-local variables are checked.
        for var in &program.globals {
            self.check_unused_var(var);
        }
        for state in &program.states {
            for var in &state.locals {
                self.check_unused_var(var);
            }
        }

        // Check for unreachable states (W0104).
        self.check_unreachable_states(program);
    }

    /// Mark a variable name as having been read somewhere in the program.
    fn mark_var_used(&mut self, name: &str) {
        self.used_vars.insert(name.to_string());
    }

    /// Emit W0103 if `var` is never read anywhere. Variables named
    /// with a leading `_` are exempt by convention.
    fn check_unused_var(&mut self, var: &VarDecl) {
        if var.name.starts_with('_') {
            return;
        }
        if self.used_vars.contains(&var.name) {
            return;
        }
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            code: ErrorCode::W0103,
            message: format!("unused variable '{}'", var.name),
            span: var.span,
            labels: Vec::<Label>::new(),
            help: Some("prefix with '_' to silence this warning, or remove the declaration".into()),
            note: None,
        });
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
            Expr::FieldAccess(name, field, span) => {
                // Resolve the struct variable and verify the field
                // exists. Mark the synthetic `name.field` variable as
                // used so W0103 doesn't fire.
                let full_name = format!("{name}.{field}");
                if self.symbols.contains_key(&full_name) {
                    self.mark_var_used(name);
                    self.mark_var_used(&full_name);
                } else if !self.symbols.contains_key(name) {
                    self.emit_undefined_var(name, *span);
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("'{name}' has no field '{field}'"),
                        *span,
                    ));
                }
            }
            Expr::BinaryOp(lhs, op, rhs, span) => {
                // W0101: warn about multiply/divide/modulo with a non-
                // constant operand. These lower to calls into the
                // software multiply/divide routines, which are far more
                // expensive than the simple inline opcodes used for
                // add/sub. A literal like `x * 2` can be strength-
                // reduced to a shift and is therefore cheap.
                if matches!(op, BinOp::Mul | BinOp::Div | BinOp::Mod)
                    && !is_small_constant(lhs)
                    && !is_small_constant(rhs)
                {
                    let op_name = match op {
                        BinOp::Mul => "multiply",
                        BinOp::Div => "divide",
                        BinOp::Mod => "modulo",
                        _ => unreachable!(),
                    };
                    self.diagnostics.push(
                        Diagnostic::warning(
                            ErrorCode::W0101,
                            format!("{op_name} with two non-constant operands is expensive"),
                            *span,
                        )
                        .with_help(
                            "consider precomputing or using a power-of-2 constant for strength reduction",
                        ),
                    );
                }
                self.walk_expr_reads(lhs);
                self.walk_expr_reads(rhs);
            }
            Expr::UnaryOp(_, inner, _) | Expr::Cast(inner, _, _) => {
                self.walk_expr_reads(inner);
            }
            Expr::Call(name, args, span) => {
                // If the function is known, validate its call signature.
                // Undefined-function errors are surfaced elsewhere (for
                // Statement::Call) and via the call-graph pass.
                if self.function_signatures.contains_key(name) {
                    self.check_call_signature(name, args, *span);
                }
                for arg in args {
                    self.walk_expr_reads(arg);
                }
            }
            Expr::ArrayLiteral(elems, _) => {
                for e in elems {
                    self.walk_expr_reads(e);
                }
            }
            Expr::StructLiteral(name, fields, span) => {
                // Validate that the struct type exists and that each
                // named field is actually declared. Missing or extra
                // fields are an error; duplicate fields are silently
                // ignored (last-writer-wins).
                if let Some(layout) = self.struct_layouts.get(name).cloned() {
                    for (fname, fexpr) in fields {
                        if let Some((_, field_type, _)) =
                            layout.fields.iter().find(|(n, _, _)| n == fname)
                        {
                            self.walk_expr_reads(fexpr);
                            self.check_expr_type(fexpr, field_type);
                        } else {
                            self.diagnostics.push(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("struct '{name}' has no field '{fname}'"),
                                *span,
                            ));
                        }
                    }
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0201,
                        format!("unknown struct type '{name}'"),
                        *span,
                    ));
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

    /// Register a struct declaration. Computes each field's byte
    /// offset from the base address (fields are laid out contiguously
    /// in declaration order with no padding), and records the total
    /// size. v1 structs only support primitive fields (u8/i8/bool).
    fn register_struct(&mut self, s: &StructDecl) {
        if self.struct_layouts.contains_key(&s.name) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0501,
                format!("duplicate struct declaration of '{}'", s.name),
                s.span,
            ));
            return;
        }
        let mut fields = Vec::new();
        let mut offset: u16 = 0;
        for field in &s.fields {
            // Reject non-primitive field types for now.
            let size = match &field.field_type {
                NesType::U8 | NesType::I8 | NesType::Bool => 1,
                _ => {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0201,
                        format!(
                            "struct field '{}' has unsupported type '{}' (only u8/i8/bool allowed)",
                            field.name, field.field_type
                        ),
                        field.span,
                    ));
                    continue;
                }
            };
            fields.push((field.name.clone(), field.field_type.clone(), offset));
            offset += size;
        }
        self.struct_layouts.insert(
            s.name.clone(),
            StructLayout {
                size: offset,
                fields,
            },
        );
    }

    /// Register each variant of an enum declaration as a `u8` constant
    /// with a value equal to its declaration order. Variant names must
    /// be globally unique; a duplicate name emits E0501.
    fn register_enum(&mut self, e: &EnumDecl) {
        if self.symbols.contains_key(&e.name) {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0501,
                format!("duplicate declaration of '{}'", e.name),
                e.span,
            ));
            // Don't return — still register the variants.
        }
        for (variant_name, variant_span) in &e.variants {
            if self.symbols.contains_key(variant_name) {
                self.diagnostics.push(Diagnostic::error(
                    ErrorCode::E0501,
                    format!("duplicate declaration of '{variant_name}'"),
                    *variant_span,
                ));
                continue;
            }
            self.symbols.insert(
                variant_name.clone(),
                Symbol {
                    name: variant_name.clone(),
                    sym_type: NesType::U8,
                    is_const: true,
                    span: *variant_span,
                },
            );
        }
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

        // Validate struct type exists before sizing.
        if let NesType::Struct(sname) = &var.var_type {
            if !self.struct_layouts.contains_key(sname) {
                self.diagnostics.push(Diagnostic::error(
                    ErrorCode::E0201,
                    format!("unknown struct type '{sname}'"),
                    var.span,
                ));
                return;
            }
        }

        let struct_sizes: HashMap<String, u16> = self
            .struct_layouts
            .iter()
            .map(|(n, l)| (n.clone(), l.size))
            .collect();
        let size = type_size_with(&var.var_type, &struct_sizes);
        let Some(address) = self.allocate_ram(size, var.span) else {
            // Allocation failed (E0301 already emitted) — still add the
            // symbol so that later references don't cascade into E0502,
            // but don't record a var_allocations entry.
            self.symbols.insert(
                var.name.clone(),
                Symbol {
                    name: var.name.clone(),
                    sym_type: var.var_type.clone(),
                    is_const: false,
                    span: var.span,
                },
            );
            return;
        };

        // For struct-typed variables, synthesize per-field entries in
        // the symbol table and var_allocations. This lets the rest of
        // the compiler treat `pos.x` and `pos.y` as ordinary variables
        // at known addresses, without special-casing struct layout.
        if let NesType::Struct(sname) = &var.var_type {
            let layout = self.struct_layouts[sname].clone();
            for (field_name, field_type, offset) in &layout.fields {
                let full_name = format!("{}.{field_name}", var.name);
                self.symbols.insert(
                    full_name.clone(),
                    Symbol {
                        name: full_name.clone(),
                        sym_type: field_type.clone(),
                        is_const: false,
                        span: var.span,
                    },
                );
                self.var_allocations.push(VarAllocation {
                    name: full_name,
                    address: address + offset,
                    size: 1,
                });
            }
            // Also register the struct variable itself (as a symbol
            // only — it doesn't have a single VarAllocation entry).
            self.symbols.insert(
                var.name.clone(),
                Symbol {
                    name: var.name.clone(),
                    sym_type: var.var_type.clone(),
                    is_const: false,
                    span: var.span,
                },
            );
            return;
        }

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
        let param_types: Vec<NesType> = fun.params.iter().map(|p| p.param_type.clone()).collect();
        self.function_signatures
            .insert(fun.name.clone(), param_types);
    }

    /// Attempt to allocate `size` bytes of RAM for a variable declared
    /// at `span`. Returns `None` on overflow, emitting E0301. The
    /// zero-page user region is bounded above by [`ZP_USER_CAP`] to
    /// leave room for IR codegen temp slots starting at $80.
    fn allocate_ram(&mut self, size: u16, span: Span) -> Option<u16> {
        // Zero-page u8 allocation — bounded by ZP_USER_CAP to avoid
        // colliding with the IR temp region at $80+.
        if size == 1 && self.next_zp_addr < ZP_USER_CAP {
            let addr = u16::from(self.next_zp_addr);
            self.next_zp_addr = self.next_zp_addr.wrapping_add(1);
            return Some(addr);
        }

        // Larger / remaining allocations go into the main RAM region
        // after the OAM buffer.
        let end = self.next_ram_addr.checked_add(size)?;
        if end > RAM_END {
            self.diagnostics.push(
                Diagnostic::error(
                    ErrorCode::E0301,
                    "out of RAM: too many variables declared",
                    span,
                )
                .with_help(
                    "the NES only has 2 KB of RAM ($0000-$07FF); consider removing some globals",
                ),
            );
            return None;
        }
        let addr = self.next_ram_addr;
        self.next_ram_addr = end;
        Some(addr)
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
        let mut terminated_by: Option<Span> = None;
        let mut warned_dead_code = false;
        for stmt in &block.statements {
            if let Some(term_span) = terminated_by {
                if !warned_dead_code {
                    self.diagnostics.push(
                        Diagnostic::warning(
                            ErrorCode::W0104,
                            "unreachable code after return / break / transition",
                            stmt.span(),
                        )
                        .with_label(term_span, "execution stops here"),
                    );
                    warned_dead_code = true;
                }
            }
            self.check_statement(stmt, state_names);
            if stmt_is_terminator(stmt) && terminated_by.is_none() {
                terminated_by = Some(stmt.span());
            }
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
                        } else {
                            // Assigning to an undeclared name is an
                            // error — the lowering would otherwise
                            // silently synthesize a VarId for it.
                            self.emit_undefined_var(name, *span);
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
                        } else {
                            self.emit_undefined_var(name, *span);
                        }
                        // Indexing an array counts as a read of the array,
                        // and the index expression itself may contain reads.
                        self.mark_var_used(name);
                        self.walk_expr_reads(idx);
                    }
                    LValue::Field(name, field) => {
                        let full_name = format!("{name}.{field}");
                        if self.symbols.contains_key(&full_name) {
                            // Assigning to a field is a mutation; don't
                            // mark the struct variable as "read" just
                            // because we wrote to one of its fields.
                            self.mark_var_used(&full_name);
                        } else if self.symbols.contains_key(name) {
                            self.diagnostics.push(Diagnostic::error(
                                ErrorCode::E0201,
                                format!("'{name}' has no field '{field}'"),
                                *span,
                            ));
                        } else {
                            self.emit_undefined_var(name, *span);
                        }
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
            Statement::For {
                var,
                start,
                end,
                body,
                span,
            } => {
                // Evaluate start/end (both u8) for reads and type
                // checking, then register the loop variable as a u8
                // for the duration of the body.
                self.walk_expr_reads(start);
                self.walk_expr_reads(end);
                self.check_expr_type(start, &NesType::U8);
                self.check_expr_type(end, &NesType::U8);
                let was_shadowed = self.symbols.remove(var);
                self.symbols.insert(
                    var.clone(),
                    Symbol {
                        name: var.clone(),
                        sym_type: NesType::U8,
                        is_const: false,
                        span: *span,
                    },
                );
                // Synthesize a VarAllocation for the loop variable
                // so IR lowering / codegen can treat it like any
                // other u8 local.
                let loop_var_addr = self.allocate_ram(1, *span).unwrap_or(0x10);
                self.var_allocations.push(VarAllocation {
                    name: var.clone(),
                    address: loop_var_addr,
                    size: 1,
                });
                // Loop variable is always "used" in the header.
                self.mark_var_used(var);
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_block(body, state_names);
                self.in_loop = was_in_loop;
                self.symbols.remove(var);
                if let Some(old) = was_shadowed {
                    self.symbols.insert(var.clone(), old);
                }
            }
            Statement::Loop(body, span) => {
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_block(body, state_names);
                self.in_loop = was_in_loop;
                // W0102: loop body must contain a break, return,
                // transition, or wait_frame — otherwise the NES spins
                // forever inside the loop and vblank never gets handled.
                if !block_can_exit_or_yield(body) {
                    self.diagnostics.push(Diagnostic::warning(
                        ErrorCode::W0102,
                        "infinite loop with no break, return, transition, or wait_frame",
                        *span,
                    ).with_help("add `wait_frame`, `break`, `return`, or `transition` somewhere in the body"));
                }
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
            Statement::Return(Some(expr), span) => {
                self.walk_expr_reads(expr);
                if let Some(ret_ty) = self.current_return_type.clone() {
                    // Function with declared return type — check the value.
                    self.check_expr_type(expr, &ret_ty);
                } else if self.in_function_body {
                    // Function with no declared return type ("void"),
                    // but the return statement has a value.
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        "return value in function with no declared return type",
                        *span,
                    ));
                }
                // State handlers (`in_function_body == false`) accept
                // `return value` silently — the value is simply discarded.
            }
            Statement::Call(name, args, span) => {
                if is_intrinsic(name) {
                    self.check_intrinsic_args(name, args, *span);
                } else if self.symbols.contains_key(name) {
                    self.check_call_signature(name, args, *span);
                } else {
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
            Statement::Return(None, span) => {
                // Bare `return` in a function with a declared return
                // type is an error — the caller expects a value.
                if self.in_function_body && self.current_return_type.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        "missing return value in function with declared return type",
                        *span,
                    ));
                }
            }
            Statement::WaitFrame(_) => {}
            Statement::DebugLog(args, _) => {
                for arg in args {
                    self.walk_expr_reads(arg);
                }
            }
            Statement::DebugAssert(cond, _) => {
                self.walk_expr_reads(cond);
                self.check_expr_type(cond, &NesType::Bool);
            }
            Statement::InlineAsm(_, _) | Statement::RawAsm(_, _) => {
                // Inline assembly is treated as an opaque block. The
                // codegen parses and validates the body; analysis has
                // nothing to check.
            }
            Statement::Play(name, span) => {
                // `play Name` is valid if the name refers to a
                // declared sfx block or to a builtin effect. Anything
                // else is an E0505 — the runtime driver needs a
                // known blob to point at, and silently accepting
                // bad names would hide typos.
                if !self.sfx_names.contains(name) && !crate::assets::is_builtin_sfx(name) {
                    self.diagnostics.push(
                        Diagnostic::error(ErrorCode::E0505, format!("unknown sfx '{name}'"), *span)
                            .with_help(
                                "declare one with `sfx Name { pitch: [..], volume: [..] }`, \
                         or use a builtin: coin, jump, hit, click, cancel, shoot, step",
                            ),
                    );
                }
            }
            Statement::StartMusic(name, span) => {
                if !self.music_names.contains(name) && !crate::assets::is_builtin_music(name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            ErrorCode::E0505,
                            format!("unknown music track '{name}'"),
                            *span,
                        )
                        .with_help(
                            "declare one with `music Name { notes: [..] }`, \
                         or use a builtin: theme, battle, victory, gameover",
                        ),
                    );
                }
            }
            Statement::StopMusic(_) => {
                // No arguments, nothing to validate.
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
            LValue::Field(name, field) => {
                let full_name = format!("{name}.{field}");
                self.symbols.get(&full_name).map(|s| s.sym_type.clone())
            }
        }
    }

    /// Check that a call site matches the function's declared signature:
    /// argument count matches the parameter count, and each argument's
    /// inferred type is compatible with the declared parameter type.
    fn check_call_signature(&mut self, name: &str, args: &[Expr], span: Span) {
        let Some(params) = self.function_signatures.get(name).cloned() else {
            return;
        };
        if params.len() != args.len() {
            self.diagnostics.push(Diagnostic::error(
                ErrorCode::E0203,
                format!(
                    "wrong number of arguments to '{name}': expected {}, got {}",
                    params.len(),
                    args.len()
                ),
                span,
            ));
            return;
        }
        for (param_ty, arg) in params.iter().zip(args.iter()) {
            self.check_expr_type(arg, param_ty);
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
            Expr::FieldAccess(name, field, _) => {
                let full_name = format!("{name}.{field}");
                self.symbols.get(&full_name).map(|s| s.sym_type.clone())
            }
            Expr::ArrayLiteral(_, _) => Some(NesType::U8), // element type inferred from context
            Expr::Cast(_, target, _) => Some(target.clone()),
            Expr::StructLiteral(name, _, _) => Some(NesType::Struct(name.clone())),
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
        Statement::For { body, .. } => {
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
        Statement::For {
            start, end, body, ..
        } => {
            collect_calls_expr(start, calls);
            collect_calls_expr(end, calls);
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
        | Statement::InlineAsm(_, _)
        | Statement::RawAsm(_, _)
        | Statement::Play(_, _)
        | Statement::StartMusic(_, _)
        | Statement::StopMusic(_) => {}
    }
}

fn collect_calls_block(block: &Block, calls: &mut Vec<String>) {
    for stmt in &block.statements {
        collect_calls_stmt(stmt, calls);
    }
}

/// Return true if the given block contains any statement that can
/// either exit the enclosing loop (`break`, `return`, `transition`)
/// or yield control back to the frame loop (`wait_frame`).
///
/// This is used by the W0102 check to decide whether an otherwise-
/// unbounded `loop { }` is actually an infinite spin. We recurse into
/// nested control-flow blocks so that a `break` inside a conditional
/// body still counts as "can exit".
/// True if the expression is a small integer literal — used to avoid
/// emitting W0101 for multiply/divide where at least one operand can be
/// handled by strength reduction (e.g. `x * 2`, `x / 4`).
fn is_small_constant(expr: &Expr) -> bool {
    matches!(expr, Expr::IntLiteral(_, _))
}

/// True if `name` is a built-in intrinsic function recognized by the
/// compiler. Intrinsics don't need a declaration and may have
/// special codegen (e.g. \`poke\` / \`peek\` write to raw addresses).
fn is_intrinsic(name: &str) -> bool {
    matches!(name, "poke" | "peek")
}

impl Analyzer {
    /// Validate the arguments to a built-in intrinsic. Emits
    /// diagnostics for mismatched arity or non-constant addresses.
    fn check_intrinsic_args(&mut self, name: &str, args: &[Expr], span: Span) {
        match name {
            "poke" => {
                if args.len() != 2 {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        format!(
                            "`poke` takes exactly 2 arguments (addr, value), got {}",
                            args.len()
                        ),
                        span,
                    ));
                }
            }
            "peek" => {
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::error(
                        ErrorCode::E0203,
                        format!("`peek` takes exactly 1 argument (addr), got {}", args.len()),
                        span,
                    ));
                }
            }
            _ => {}
        }
    }
}

/// True if this statement unconditionally ends block execution —
/// subsequent statements in the same block cannot be reached.
fn stmt_is_terminator(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::Return(_, _)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Transition(_, _)
    )
}

fn block_can_exit_or_yield(block: &Block) -> bool {
    block.statements.iter().any(stmt_can_exit_or_yield)
}

fn stmt_can_exit_or_yield(stmt: &Statement) -> bool {
    match stmt {
        Statement::Break(_)
        | Statement::Return(_, _)
        | Statement::Transition(_, _)
        | Statement::WaitFrame(_) => true,
        Statement::If(_, then_b, elifs, else_b, _) => {
            block_can_exit_or_yield(then_b)
                || elifs.iter().any(|(_, b)| block_can_exit_or_yield(b))
                || else_b.as_ref().is_some_and(block_can_exit_or_yield)
        }
        Statement::While(_, body, _) | Statement::Loop(body, _) => {
            // A nested loop with a wait_frame inside still yields
            // control, so check its body recursively.
            block_can_exit_or_yield(body)
        }
        Statement::For { body, .. } => block_can_exit_or_yield(body),
        _ => false,
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
        Expr::StructLiteral(_, fields, _) => {
            for (_, e) in fields {
                collect_calls_expr(e, calls);
            }
        }
        Expr::Cast(inner, _, _) => {
            collect_calls_expr(inner, calls);
        }
        Expr::IntLiteral(_, _)
        | Expr::BoolLiteral(_, _)
        | Expr::Ident(_, _)
        | Expr::FieldAccess(_, _, _)
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

/// Compute the byte size of a type. Struct types are looked up in
/// `struct_sizes`; if absent, returns 0 (the analyzer will have
/// reported an error already).
fn type_size_with(t: &NesType, struct_sizes: &HashMap<String, u16>) -> u16 {
    match t {
        NesType::U8 | NesType::I8 | NesType::Bool => 1,
        NesType::U16 => 2,
        NesType::Array(elem, count) => type_size_with(elem, struct_sizes) * count,
        NesType::Struct(name) => struct_sizes.get(name).copied().unwrap_or(0),
    }
}

fn is_integer_type(t: &NesType) -> bool {
    matches!(t, NesType::U8 | NesType::I8 | NesType::U16)
}
