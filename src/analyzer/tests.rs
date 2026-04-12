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
