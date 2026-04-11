use super::*;
use crate::analyzer;
use crate::parser;

fn lower_ok(input: &str) -> IrProgram {
    let (prog, diags) = parser::parse(input);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "analysis errors: {:?}",
        analysis.diagnostics
    );
    lower(&prog, &analysis)
}

#[test]
fn lower_minimal_program() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        on frame { px = 1 }
        start Main
    "#,
    );
    assert_eq!(ir.globals.len(), 1);
    assert_eq!(ir.globals[0].name, "px");
    assert_eq!(ir.globals[0].init_value, Some(128));
    // Should have at least one function (the frame handler)
    assert!(!ir.functions.is_empty());
}

#[test]
fn lower_var_assignment() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x = 42 }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    // Should have a StoreVar op
    let has_store = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::StoreVar(..)));
    assert!(has_store, "should emit StoreVar for assignment");
}

#[test]
fn lower_plus_assign() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x += 5 }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    let has_add = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::Add(..)));
    assert!(has_add, "should emit Add for += operator");
}

#[test]
fn lower_if_creates_branch() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 { x = 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    let has_branch = frame_fn
        .blocks
        .iter()
        .any(|b| matches!(&b.terminator, IrTerminator::Branch(..)));
    assert!(has_branch, "if statement should produce a Branch terminator");
}

#[test]
fn lower_while_creates_loop() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            while x < 10 { x += 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    // A while loop needs at least 3 blocks: condition check, body, and exit
    assert!(
        frame_fn.blocks.len() >= 3,
        "while should create multiple blocks, got {}",
        frame_fn.blocks.len()
    );
}

#[test]
fn lower_button_read() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame {
            if button.right { px += 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    let has_input = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::ReadInput));
    assert!(has_input, "button read should emit ReadInput op");
}

#[test]
fn lower_draw_sprite() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame { draw Smiley at: (px, py) }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    let has_draw = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::DrawSprite { .. }));
    assert!(has_draw, "should emit DrawSprite op");
}

#[test]
fn lower_constants_become_immediates() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 3
        var px: u8 = 0
        on frame { px += SPEED }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    // SPEED should be lowered to LoadImm(_, 3)
    let has_imm3 = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::LoadImm(_, 3)));
    assert!(has_imm3, "constant should be inlined as LoadImm");
}

#[test]
fn lower_multiple_states() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        state Title {
            on enter { wait_frame }
            on frame { wait_frame }
        }
        state Game {
            on frame { wait_frame }
        }
        start Title
    "#,
    );
    // Should have: Title_enter, Title_frame, Game_frame
    assert!(
        ir.functions.len() >= 3,
        "should have at least 3 functions for 2 states, got {}",
        ir.functions.len()
    );
    let names: Vec<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("Title_enter")),
        "should have Title_enter handler"
    );
    assert!(
        names.iter().any(|n| n.contains("Title_frame")),
        "should have Title_frame handler"
    );
    assert!(
        names.iter().any(|n| n.contains("Game_frame")),
        "should have Game_frame handler"
    );
}

#[test]
fn lower_op_count() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x = 1 }
        start Main
    "#,
    );
    assert!(ir.op_count() > 0, "should have some IR ops");
}

#[test]
fn lower_wait_frame() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        on frame { wait_frame }
        start Main
    "#,
    );
    let frame_fn = ir.functions.iter().find(|f| f.name.contains("frame")).unwrap();
    let has_wait = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::WaitFrame));
    assert!(has_wait, "should emit WaitFrame op");
}
