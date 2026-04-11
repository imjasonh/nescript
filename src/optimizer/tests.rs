use super::*;
use crate::ir::{IrBasicBlock, IrFunction, IrOp, IrProgram, IrTemp, IrTerminator, VarId};
use crate::lexer::Span;

/// Helper: build a minimal `IrProgram` wrapping a single function with one block.
fn make_program(ops: Vec<IrOp>, terminator: IrTerminator) -> IrProgram {
    IrProgram {
        functions: vec![IrFunction {
            name: "test_fn".to_string(),
            blocks: vec![IrBasicBlock {
                label: "entry".to_string(),
                ops,
                terminator,
            }],
            locals: vec![],
            param_count: 0,
            has_return: false,
            source_span: Span::new(0, 0, 0),
        }],
        globals: vec![],
        rom_data: vec![],
    }
}

// ---------------------------------------------------------------------------
// Constant folding tests
// ---------------------------------------------------------------------------

#[test]
fn fold_add_constants() {
    // LoadImm(t0, 3), LoadImm(t1, 5), Add(t2, t0, t1)
    // should become LoadImm(t2, 8) with unused t0/t1 removed.
    let t0 = IrTemp(0);
    let t1 = IrTemp(1);
    let t2 = IrTemp(2);

    let ops = vec![
        IrOp::LoadImm(t0, 3),
        IrOp::LoadImm(t1, 5),
        IrOp::Add(t2, t0, t1),
    ];
    let mut prog = make_program(ops, IrTerminator::Return(Some(t2)));
    optimize(&mut prog);

    let block = &prog.functions[0].blocks[0];
    // After folding + dead-code removal we should have exactly one LoadImm(t2, 8).
    assert_eq!(block.ops.len(), 1, "expected 1 op, got {:?}", block.ops);
    assert!(
        matches!(block.ops[0], IrOp::LoadImm(t, 8) if t == t2),
        "expected LoadImm(t2, 8), got {:?}",
        block.ops[0]
    );
}

#[test]
fn fold_sub_constants() {
    let t0 = IrTemp(0);
    let t1 = IrTemp(1);
    let t2 = IrTemp(2);

    let ops = vec![
        IrOp::LoadImm(t0, 10),
        IrOp::LoadImm(t1, 3),
        IrOp::Sub(t2, t0, t1),
    ];
    let mut prog = make_program(ops, IrTerminator::Return(Some(t2)));
    optimize(&mut prog);

    let block = &prog.functions[0].blocks[0];
    assert_eq!(block.ops.len(), 1, "expected 1 op, got {:?}", block.ops);
    assert!(
        matches!(block.ops[0], IrOp::LoadImm(t, 7) if t == t2),
        "expected LoadImm(t2, 7), got {:?}",
        block.ops[0]
    );
}

#[test]
fn fold_comparison() {
    // CmpEq with two equal constants should fold to LoadImm(dest, 1).
    let t0 = IrTemp(0);
    let t1 = IrTemp(1);
    let t2 = IrTemp(2);

    let ops = vec![
        IrOp::LoadImm(t0, 42),
        IrOp::LoadImm(t1, 42),
        IrOp::CmpEq(t2, t0, t1),
    ];
    let mut prog = make_program(ops, IrTerminator::Return(Some(t2)));
    optimize(&mut prog);

    let block = &prog.functions[0].blocks[0];
    assert_eq!(block.ops.len(), 1, "expected 1 op, got {:?}", block.ops);
    assert!(
        matches!(block.ops[0], IrOp::LoadImm(t, 1) if t == t2),
        "expected LoadImm(t2, 1), got {:?}",
        block.ops[0]
    );

    // CmpEq with two different constants should fold to LoadImm(dest, 0).
    let ops2 = vec![
        IrOp::LoadImm(t0, 10),
        IrOp::LoadImm(t1, 20),
        IrOp::CmpEq(t2, t0, t1),
    ];
    let mut prog2 = make_program(ops2, IrTerminator::Return(Some(t2)));
    optimize(&mut prog2);

    let block2 = &prog2.functions[0].blocks[0];
    assert_eq!(block2.ops.len(), 1, "expected 1 op, got {:?}", block2.ops);
    assert!(
        matches!(block2.ops[0], IrOp::LoadImm(t, 0) if t == t2),
        "expected LoadImm(t2, 0), got {:?}",
        block2.ops[0]
    );
}

#[test]
fn dead_code_removes_unused() {
    // t0 is loaded but never used anywhere — should be eliminated.
    let t0 = IrTemp(0);
    let t1 = IrTemp(1);

    let ops = vec![
        IrOp::LoadImm(t0, 99), // dead — t0 never referenced
        IrOp::LoadImm(t1, 42), // alive — returned
    ];
    let mut prog = make_program(ops, IrTerminator::Return(Some(t1)));
    optimize(&mut prog);

    let block = &prog.functions[0].blocks[0];
    assert_eq!(block.ops.len(), 1, "expected 1 op, got {:?}", block.ops);
    assert!(
        matches!(block.ops[0], IrOp::LoadImm(t, 42) if t == t1),
        "expected only LoadImm(t1, 42), got {:?}",
        block.ops[0]
    );
}

#[test]
fn optimize_preserves_used_ops() {
    // Every op here is live — nothing should be removed.
    let t0 = IrTemp(0);
    let t1 = IrTemp(1);
    let v0 = VarId(0);

    let ops = vec![
        IrOp::LoadImm(t0, 5),
        IrOp::LoadVar(t1, v0),
        IrOp::StoreVar(v0, t0),
    ];
    let mut prog = make_program(ops.clone(), IrTerminator::Return(Some(t1)));
    optimize(&mut prog);

    let block = &prog.functions[0].blocks[0];
    // LoadImm(t0, 5) is used by StoreVar; LoadVar(t1, v0) is used by Return.
    // StoreVar has no dest so it's always kept.
    assert_eq!(block.ops.len(), 3, "expected 3 ops, got {:?}", block.ops);
}
