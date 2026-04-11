use super::*;
use crate::parser::ast::*;

fn parse_ok(input: &str) -> Program {
    let (prog, diags) = parse(input);
    assert!(
        diags.is_empty(),
        "unexpected errors: {diags:?}\ninput: {input}"
    );
    prog.expect("expected successful parse")
}

fn parse_err(input: &str) -> Vec<crate::errors::ErrorCode> {
    let (_, diags) = parse(input);
    assert!(!diags.is_empty(), "expected errors but got none");
    diags.into_iter().map(|d| d.code).collect()
}

const MINIMAL_GAME: &str = r#"
game "Test" { mapper: NROM }
var px: u8 = 0
on frame {
    px = 1
}
start Main
"#;

// ── Game declaration ──

#[test]
fn parse_game_decl() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.game.name, "Test");
    assert_eq!(prog.game.mapper, Mapper::NROM);
}

#[test]
fn parse_game_with_mirroring() {
    let src = r#"
        game "Test" { mapper: NROM mirroring: vertical }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.mirroring, Mirroring::Vertical);
}

// ── Variables ──

#[test]
fn parse_global_var() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.globals.len(), 1);
    assert_eq!(prog.globals[0].name, "px");
    assert_eq!(prog.globals[0].var_type, NesType::U8);
}

#[test]
fn parse_multiple_globals() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        var py: u8 = 120
        on frame { px = 1 }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.globals.len(), 2);
    assert_eq!(prog.globals[0].name, "px");
    assert_eq!(prog.globals[1].name, "py");
}

// ── Constants ──

#[test]
fn parse_const_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 2
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.constants.len(), 1);
    assert_eq!(prog.constants[0].name, "SPEED");
}

// ── On frame ──

#[test]
fn parse_on_frame_implicit_state() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.states.len(), 1);
    assert_eq!(prog.states[0].name, "Main");
    assert!(prog.states[0].on_frame.is_some());
    assert_eq!(prog.start_state, "Main");
}

// ── Statements ──

#[test]
fn parse_if_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        on frame {
            if button.right { px += 2 }
        }
        var px: u8 = 0
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 1);
    assert!(matches!(frame.statements[0], Statement::If(..)));
}

#[test]
fn parse_if_else() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                x = 1
            } else {
                x = 0
            }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, _, else_block, _) => {
            assert!(else_block.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn parse_if_else_if() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                x = 1
            } else if x == 1 {
                x = 2
            } else {
                x = 0
            }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, else_ifs, else_block, _) => {
            assert_eq!(else_ifs.len(), 1);
            assert!(else_block.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn parse_while_loop() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            while x < 10 { x += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert!(matches!(frame.statements[0], Statement::While(..)));
}

#[test]
fn parse_draw_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame {
            draw Smiley at: (px, py)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Draw(d) => {
            assert_eq!(d.sprite_name, "Smiley");
        }
        other => panic!("expected Draw, got {other:?}"),
    }
}

#[test]
fn parse_draw_with_frame() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame {
            draw Smiley at: (px, py) frame: 0
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Draw(d) => {
            assert!(d.frame.is_some());
        }
        other => panic!("expected Draw, got {other:?}"),
    }
}

// ── Expressions ──

#[test]
fn parse_arithmetic() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            x = 1 + 2 * 3
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    // x = 1 + (2 * 3) -- multiplication binds tighter
    match &frame.statements[0] {
        Statement::Assign(_, _, Expr::BinaryOp(_, BinOp::Add, right, _), _) => {
            assert!(matches!(
                right.as_ref(),
                Expr::BinaryOp(_, BinOp::Mul, _, _)
            ));
        }
        other => panic!("expected assignment with Add, got {other:?}"),
    }
}

#[test]
fn parse_button_read() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame {
            if button.right { px += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::ButtonRead(_, button, _), _, _, _, _) => {
            assert_eq!(button, "right");
        }
        other => panic!("expected if with button read, got {other:?}"),
    }
}

#[test]
fn parse_comparison_ops() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x >= 10 { x = 0 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::BinaryOp(_, BinOp::GtEq, _, _), _, _, _, _) => {}
        other => panic!("expected if with GtEq, got {other:?}"),
    }
}

// ── State declarations ──

#[test]
fn parse_explicit_state() {
    let src = r#"
        game "Test" { mapper: NROM }
        state Title {
            on enter { wait_frame }
            on frame { wait_frame }
        }
        start Title
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.states.len(), 1);
    assert_eq!(prog.states[0].name, "Title");
    assert!(prog.states[0].on_enter.is_some());
    assert!(prog.states[0].on_frame.is_some());
}

// ── Error cases ──

#[test]
fn parse_missing_game() {
    use crate::errors::ErrorCode;
    let codes = parse_err("var x: u8 = 0\nstart Main");
    assert!(codes.contains(&ErrorCode::E0504));
}

#[test]
fn parse_missing_start_with_explicit_state() {
    use crate::errors::ErrorCode;
    // Explicit state without `start` declaration should fail
    let codes = parse_err(
        r#"
        game "T" { mapper: NROM }
        state Foo { on frame { wait_frame } }
    "#,
    );
    assert!(codes.contains(&ErrorCode::E0504));
}

#[test]
fn parse_on_frame_implies_start() {
    // Top-level `on frame` auto-creates implicit "Main" state and start
    let prog = parse_ok(r#"game "T" { mapper: NROM } on frame { wait_frame }"#);
    assert_eq!(prog.start_state, "Main");
}

// ── Full M1 program ──

#[test]
fn parse_full_m1_program() {
    let src = r#"
        game "Hello Sprite" {
            mapper: NROM
        }

        var px: u8 = 128
        var py: u8 = 120

        on frame {
            if button.right { px += 2 }
            if button.left  { px -= 2 }
            if button.down  { py += 2 }
            if button.up    { py -= 2 }

            draw Smiley at: (px, py)
        }

        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.name, "Hello Sprite");
    assert_eq!(prog.globals.len(), 2);
    assert_eq!(prog.states.len(), 1);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 5); // 4 ifs + 1 draw
}
