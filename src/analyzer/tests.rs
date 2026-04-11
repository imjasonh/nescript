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
