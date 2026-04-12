use super::*;
use crate::errors::ErrorCode;
use crate::parser;

fn analyze_ok(input: &str) -> AnalysisResult {
    let (prog, diags) = parser::parse(input);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let result = analyze(&prog);
    assert!(
        result.diagnostics.iter().all(|d| !d.is_error()),
        "analysis errors: {:?}",
        result.diagnostics
    );
    result
}

fn analyze_errors(input: &str) -> Vec<ErrorCode> {
    let (prog, parse_diags) = parser::parse(input);
    if prog.is_none() {
        return parse_diags.into_iter().map(|d| d.code).collect();
    }
    let result = analyze(&prog.unwrap());
    result.diagnostics.into_iter().map(|d| d.code).collect()
}

#[test]
fn analyze_minimal_program() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        on frame { px = 1 }
        start Main
    "#,
    );
    assert!(result.symbols.contains_key("px"));
    assert_eq!(result.var_allocations.len(), 1);
}

#[test]
fn analyze_allocates_zero_page() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        var y: u8 = 0
        on frame { x = 1 }
        start Main
    "#,
    );
    // u8 vars should be allocated in zero page starting at $10
    assert_eq!(result.var_allocations[0].address, 0x10);
    assert_eq!(result.var_allocations[1].address, 0x11);
}

#[test]
fn analyze_duplicate_var() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        var x: u8 = 1
        on frame { x = 1 }
        start Main
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0501));
}

#[test]
fn analyze_undefined_transition() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { transition Nonexistent }
        }
        start Main
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0404));
}

#[test]
fn analyze_valid_transition() {
    let _result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { transition Other }
        }
        state Other {
            on frame { wait_frame }
        }
        start Main
    "#,
    );
}

#[test]
fn analyze_start_state_exists() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { wait_frame }
        }
        start Nonexistent
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0404));
}

#[test]
fn analyze_const_symbol() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 2
        var px: u8 = 0
        on frame { px = SPEED }
        start Main
    "#,
    );
    let sym = result.symbols.get("SPEED").unwrap();
    assert!(sym.is_const);
}

#[test]
fn analyze_function_registered() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        fun add(a: u8, b: u8) -> u8 { return a }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(result.symbols.contains_key("add"));
}

#[test]
fn analyze_recursion_detected() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun a() { a() }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0402));
}

#[test]
fn analyze_mutual_recursion() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun a() { b() }
        fun b() { a() }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0402));
}

#[test]
fn analyze_call_depth_ok() {
    // 3 levels of nesting — well within the default limit of 8
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        fun c() { wait_frame }
        fun b() { c() }
        fun a() { b() }
        on frame { a() }
        start Main
    "#,
    );
    // The frame handler's depth should be <= 8
    for &depth in result.max_depths.values() {
        assert!(depth <= 8, "depth {depth} should be within limit");
    }
}

#[test]
fn analyze_call_depth_exceeded() {
    // Build a call chain deeper than 8: f1 -> f2 -> ... -> f10
    let result = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun f10() { wait_frame }
        fun f9() { f10() }
        fun f8() { f9() }
        fun f7() { f8() }
        fun f6() { f7() }
        fun f5() { f6() }
        fun f4() { f5() }
        fun f3() { f4() }
        fun f2() { f3() }
        fun f1() { f2() }
        on frame { f1() }
        start Main
    "#,
    );
    assert!(
        result.contains(&ErrorCode::E0401),
        "expected E0401 for exceeded call depth, got: {result:?}"
    );
}

#[test]
fn analyze_undefined_function() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        on frame { no_such_fn() }
        start Main
    "#,
    );
    assert!(errors.contains(&ErrorCode::E0503));
}

#[test]
fn analyze_call_arity_mismatch() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun add(a: u8, b: u8) -> u8 { return a }
        on frame { add(1) }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "calling with wrong argument count should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_call_arity_ok() {
    analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        fun add(a: u8, b: u8) -> u8 { return a }
        on frame { add(1, 2) }
        start Main
    "#,
    );
}

#[test]
fn analyze_call_arity_in_expr_context() {
    // Calls used as expressions should also be checked.
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun two(a: u8, b: u8) -> u8 { return a }
        var x: u8 = 0
        on frame { x = two(1) }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "call arity error in expression context should still trigger E0203: {errors:?}"
    );
}

#[test]
fn analyze_return_type_ok() {
    analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        fun get_five() -> u8 { return 5 }
        on frame { wait_frame }
        start Main
    "#,
    );
}

#[test]
fn analyze_return_wrong_type() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun is_ok() -> bool { return 5 }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0201),
        "returning wrong type should produce E0201, got: {errors:?}"
    );
}

#[test]
fn analyze_struct_variable_allocates_fields() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2
        on frame {
            pos.x = 10
            pos.y = pos.x
        }
        start Main
    "#,
    );
    // The analyzer should synthesize pos.x and pos.y as separate
    // variables with consecutive addresses.
    let px = result
        .var_allocations
        .iter()
        .find(|a| a.name == "pos.x")
        .expect("pos.x should be allocated");
    let py = result
        .var_allocations
        .iter()
        .find(|a| a.name == "pos.y")
        .expect("pos.y should be allocated");
    assert_eq!(py.address, px.address + 1);
}

#[test]
fn analyze_struct_unknown_field_errors() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2
        on frame { pos.z = 5 }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0201),
        "unknown field should emit E0201: {errors:?}"
    );
}

#[test]
fn analyze_unknown_struct_type_errors() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        var pos: NoSuchStruct
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0201),
        "unknown struct type should emit E0201: {errors:?}"
    );
}

#[test]
fn analyze_assign_to_undefined_var_errors() {
    // Assigning to an undeclared variable must produce E0502
    // rather than silently creating the variable.
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        on frame { nope = 5 }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0502),
        "assignment to undefined var should produce E0502, got: {errors:?}"
    );
}

#[test]
fn analyze_enum_variants_as_constants() {
    let result = analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        enum Color { Red, Green, Blue }
        var c: u8 = Red
        on frame {
            if c == Blue { c = Green }
        }
        start Main
    "#,
    );
    // Variants should be registered as constant symbols.
    assert!(result.symbols.get("Red").is_some_and(|s| s.is_const));
    assert!(result.symbols.get("Green").is_some_and(|s| s.is_const));
    assert!(result.symbols.get("Blue").is_some_and(|s| s.is_const));
}

#[test]
fn analyze_duplicate_enum_variant_errors() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        enum A { Foo, Bar }
        enum B { Baz, Bar }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0501),
        "duplicate variant should emit E0501, got: {errors:?}"
    );
}

#[test]
fn analyze_dead_code_after_break() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            loop {
                break
                x += 1
            }
        }
        start Main
    "#;
    let errors = analyze_errors(src);
    assert!(
        errors.contains(&ErrorCode::W0104),
        "code after break should trigger W0104, got: {errors:?}"
    );
}

#[test]
fn analyze_dead_code_after_transition() {
    let src = r#"
        game "Test" { mapper: NROM }
        state A {
            on frame {
                transition B
                wait_frame
            }
        }
        state B { on frame { wait_frame } }
        start A
    "#;
    let errors = analyze_errors(src);
    assert!(
        errors.contains(&ErrorCode::W0104),
        "code after transition should trigger W0104, got: {errors:?}"
    );
}

#[test]
fn analyze_dead_code_after_return_in_fn() {
    let src = r#"
        game "Test" { mapper: NROM }
        fun foo() -> u8 {
            return 5
            return 6
        }
        on frame { wait_frame }
        start Main
    "#;
    let errors = analyze_errors(src);
    assert!(
        errors.contains(&ErrorCode::W0104),
        "code after return should trigger W0104, got: {errors:?}"
    );
}

#[test]
fn analyze_ram_overflow_emits_e0301() {
    // Two arrays totalling >2 KB cannot fit in NES RAM, triggering
    // E0301 at allocation time.
    let src = r#"
        game "Test" { mapper: NROM }
        var huge: u8[2000]
        var also_huge: u8[2000]
        on frame { wait_frame }
        start Main
    "#;
    let errors = analyze_errors(src);
    assert!(
        errors.contains(&ErrorCode::E0301),
        "RAM overflow should produce E0301, got: {errors:?}"
    );
}

#[test]
fn analyze_expensive_multiply_warns() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        var a: u8 = 3
        var b: u8 = 5
        var c: u8 = 0
        on frame { c = a * b }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::W0101),
        "variable*variable multiply should emit W0101, got: {errors:?}"
    );
}

#[test]
fn analyze_multiply_by_constant_ok() {
    // Multiply by a literal is cheap (strength reduced to shifts).
    analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        var a: u8 = 3
        var c: u8 = 0
        on frame { c = a * 4 }
        start Main
    "#,
    );
}

#[test]
fn analyze_on_scanline_requires_mmc3() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { wait_frame }
            on scanline(120) { scroll(0, 0) }
        }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "on scanline without MMC3 should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_on_scanline_mmc3_ok() {
    analyze_ok(
        r#"
        game "Test" { mapper: MMC3 }
        state Main {
            on frame { wait_frame }
            on scanline(120) { scroll(0, 0) }
        }
        start Main
    "#,
    );
}

#[test]
fn analyze_loop_without_exit_warns() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            loop { x += 1 }
        }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::W0102),
        "infinite loop with no exit should produce W0102, got: {errors:?}"
    );
}

#[test]
fn analyze_loop_with_wait_frame_ok() {
    analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        on frame {
            loop { wait_frame }
        }
        start Main
    "#,
    );
}

#[test]
fn analyze_loop_with_break_ok() {
    analyze_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            loop {
                x += 1
                if x == 10 { break }
            }
        }
        start Main
    "#,
    );
}

#[test]
fn analyze_bare_return_from_typed_fn_errors() {
    // A `return` with no value inside a function that has a declared
    // return type should produce E0203.
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun get_five() -> u8 {
            return
        }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "bare return from typed fn should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_return_value_from_void_fn() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        fun do_nothing() { return 5 }
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "returning value from void function should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_const_assignment_error() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 2
        on frame { SPEED = 5 }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "assigning to const should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_break_outside_loop() {
    let errors = analyze_errors(
        r#"
        game "Test" { mapper: NROM }
        on frame { break }
        start Main
    "#,
    );
    assert!(
        errors.contains(&ErrorCode::E0203),
        "break outside loop should produce E0203, got: {errors:?}"
    );
}

#[test]
fn analyze_unused_variable_warning() {
    // `ghost` is declared but never read (only the initializer runs).
    // It should trigger a W0103 warning.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        var ghost: u8 = 0
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        result.diagnostics.iter().any(|d| d.code == ErrorCode::W0103
            && d.level == crate::errors::Level::Warning
            && d.message.contains("ghost")),
        "expected W0103 for unused var 'ghost', got: {:?}",
        result.diagnostics
    );
    // And no hard errors.
    assert!(
        result.diagnostics.iter().all(|d| !d.is_error()),
        "unexpected hard errors: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_unused_state_local_warning() {
    // State-local `bonus` is declared but never read — W0103 should fire.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            var bonus: u8 = 0
            on frame { wait_frame }
        }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::W0103 && d.message.contains("bonus")),
        "expected W0103 for unused state-local 'bonus', got: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_unused_variable_no_warning_when_read() {
    // `counter` is both written and read (in the `if` condition),
    // so W0103 should NOT fire for it.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        var counter: u8 = 0
        on frame {
            counter = counter + 1
            if counter > 60 { wait_frame }
        }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::W0103 && d.message.contains("counter")),
        "did not expect W0103 for read variable 'counter', got: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_unused_variable_underscore_prefix_silences() {
    // A leading underscore silences the W0103 warning, matching Rust's
    // convention for intentionally-unused names.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        var _reserved: u8 = 0
        on frame { wait_frame }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::W0103),
        "did not expect W0103 for underscore-prefixed var, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_unreachable_state_warning() {
    // `Orphan` is never reached from `Main` — W0104 should fire.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { wait_frame }
        }
        state Orphan {
            on frame { wait_frame }
        }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::W0104 && d.message.contains("Orphan")),
        "expected W0104 for unreachable state 'Orphan', got: {:?}",
        result.diagnostics
    );
    // And no hard errors.
    assert!(
        result.diagnostics.iter().all(|d| !d.is_error()),
        "unexpected hard errors: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_reachable_state_no_warning() {
    // Both states are reachable: Main transitions to Other, and Other
    // transitions back to Main. Neither should trigger W0104.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        state Main {
            on frame { transition Other }
        }
        state Other {
            on frame { transition Main }
        }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::W0104),
        "did not expect any W0104 warnings, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn analyze_undefined_variable_emits_e0502() {
    // `ghosy` does not exist; analyzer should emit E0502 and — thanks to
    // the suggestion helper — hint at `ghost` which is the close match.
    let (prog, diags) = parser::parse(
        r#"
        game "Test" { mapper: NROM }
        var ghost: u8 = 0
        var score: u8 = 0
        on frame {
            score = ghosy + 1
        }
        start Main
    "#,
    );
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let result = analyze(&prog.unwrap());
    let diag = result
        .diagnostics
        .iter()
        .find(|d| d.code == ErrorCode::E0502)
        .expect("expected E0502 for undefined variable 'ghosy'");
    assert!(
        diag.message.contains("ghosy"),
        "E0502 message should mention 'ghosy', got: {}",
        diag.message
    );
    assert_eq!(
        diag.help.as_deref(),
        Some("did you mean 'ghost'?"),
        "expected suggestion for 'ghost', got: {:?}",
        diag.help
    );
}
