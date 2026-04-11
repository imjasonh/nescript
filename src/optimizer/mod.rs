#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::ir::{IrBasicBlock, IrFunction, IrOp, IrProgram, IrTemp, IrTerminator};

/// Run all optimization passes on the IR program.
pub fn optimize(program: &mut IrProgram) {
    const_fold(program);
    dead_code(program);
}

// ---------------------------------------------------------------------------
// Constant folding
// ---------------------------------------------------------------------------

/// Single-pass constant folding within each basic block.
///
/// When we see `LoadImm(t, v)`, we record `t -> v`. When a binary op or
/// comparison has both operands as known constants we replace the instruction
/// with a single `LoadImm`. After folding we remove `LoadImm` ops whose
/// destination temps are no longer referenced anywhere in the block.
fn const_fold(program: &mut IrProgram) {
    for func in &mut program.functions {
        for block in &mut func.blocks {
            const_fold_block(block);
        }
    }
}

fn const_fold_block(block: &mut IrBasicBlock) {
    let mut constants: HashMap<IrTemp, u8> = HashMap::new();

    // First pass: fold arithmetic / comparison ops into LoadImm where possible.
    for op in &mut block.ops {
        match *op {
            IrOp::LoadImm(t, v) => {
                constants.insert(t, v);
            }
            IrOp::Add(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = va.wrapping_add(vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::Sub(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = va.wrapping_sub(vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::And(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = va & vb;
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::Or(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = va | vb;
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::Xor(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = va ^ vb;
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpEq(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va == vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpNe(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va != vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpLt(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va < vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpGt(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va > vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpLtEq(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va <= vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            IrOp::CmpGtEq(dest, a, b) => {
                if let (Some(&va), Some(&vb)) = (constants.get(&a), constants.get(&b)) {
                    let result = u8::from(va >= vb);
                    *op = IrOp::LoadImm(dest, result);
                    constants.insert(dest, result);
                }
            }
            _ => {}
        }
    }

    // Second pass: remove LoadImm ops whose dest temps are no longer referenced
    // as source operands by anything else in the block (ops + terminator).
    let used = collect_used_temps_in_block(block);
    block.ops.retain(|op| {
        if let IrOp::LoadImm(t, _) = op {
            used.contains(t)
        } else {
            true
        }
    });
}

// ---------------------------------------------------------------------------
// Dead code elimination
// ---------------------------------------------------------------------------

/// Remove ops whose destination temps are never used, and remove unreachable
/// basic blocks (those that have no incoming edges, except the entry block).
fn dead_code(program: &mut IrProgram) {
    for func in &mut program.functions {
        dead_code_eliminate(func);
        remove_unreachable_blocks(func);
    }
}

fn dead_code_eliminate(func: &mut IrFunction) {
    let used = collect_used_temps(func);
    for block in &mut func.blocks {
        block.ops.retain(|op| {
            if let Some(dest) = op_dest(op) {
                used.contains(&dest)
            } else {
                true // ops without a dest (StoreVar, WaitFrame, etc.) are always kept
            }
        });
    }
}

/// Collect every `IrTemp` that is used as a *source* operand anywhere in the
/// function (ops + terminators).
fn collect_used_temps(func: &IrFunction) -> HashSet<IrTemp> {
    let mut used = HashSet::new();
    for block in &func.blocks {
        collect_used_from_block(block, &mut used);
    }
    used
}

/// Collect temps used as source operands within a single block (ops + terminator).
fn collect_used_temps_in_block(block: &IrBasicBlock) -> HashSet<IrTemp> {
    let mut used = HashSet::new();
    collect_used_from_block(block, &mut used);
    used
}

fn collect_used_from_block(block: &IrBasicBlock, used: &mut HashSet<IrTemp>) {
    for op in &block.ops {
        collect_source_temps(op, used);
    }
    match &block.terminator {
        IrTerminator::Branch(t, _, _) | IrTerminator::Return(Some(t)) => {
            used.insert(*t);
        }
        IrTerminator::Jump(_) | IrTerminator::Return(None) | IrTerminator::Unreachable => {}
    }
}

/// Add all source-operand temps of an op to `used`.
fn collect_source_temps(op: &IrOp, used: &mut HashSet<IrTemp>) {
    match op {
        IrOp::LoadImm(_, _) => {} // dest only, no source temps
        IrOp::LoadVar(_, _) => {} // dest only
        IrOp::StoreVar(_, src) => {
            used.insert(*src);
        }
        IrOp::Add(_, a, b)
        | IrOp::Sub(_, a, b)
        | IrOp::Mul(_, a, b)
        | IrOp::And(_, a, b)
        | IrOp::Or(_, a, b)
        | IrOp::Xor(_, a, b)
        | IrOp::CmpEq(_, a, b)
        | IrOp::CmpNe(_, a, b)
        | IrOp::CmpLt(_, a, b)
        | IrOp::CmpGt(_, a, b)
        | IrOp::CmpLtEq(_, a, b)
        | IrOp::CmpGtEq(_, a, b) => {
            used.insert(*a);
            used.insert(*b);
        }
        IrOp::ShiftLeft(_, src, _) | IrOp::ShiftRight(_, src, _) => {
            used.insert(*src);
        }
        IrOp::Negate(_, src) | IrOp::Complement(_, src) => {
            used.insert(*src);
        }
        IrOp::ArrayLoad(_, _, idx) => {
            used.insert(*idx);
        }
        IrOp::ArrayStore(_, idx, val) => {
            used.insert(*idx);
            used.insert(*val);
        }
        IrOp::Call(_, _, args) => {
            for a in args {
                used.insert(*a);
            }
        }
        IrOp::DrawSprite { x, y, frame, .. } => {
            used.insert(*x);
            used.insert(*y);
            if let Some(f) = frame {
                used.insert(*f);
            }
        }
        IrOp::ReadInput | IrOp::WaitFrame | IrOp::Transition(_) | IrOp::SourceLoc(_) => {}
    }
}

/// Return the destination temp of an op, if it has one.
fn op_dest(op: &IrOp) -> Option<IrTemp> {
    match op {
        IrOp::LoadImm(d, _)
        | IrOp::LoadVar(d, _)
        | IrOp::Add(d, _, _)
        | IrOp::Sub(d, _, _)
        | IrOp::Mul(d, _, _)
        | IrOp::And(d, _, _)
        | IrOp::Or(d, _, _)
        | IrOp::Xor(d, _, _)
        | IrOp::ShiftLeft(d, _, _)
        | IrOp::ShiftRight(d, _, _)
        | IrOp::Negate(d, _)
        | IrOp::Complement(d, _)
        | IrOp::CmpEq(d, _, _)
        | IrOp::CmpNe(d, _, _)
        | IrOp::CmpLt(d, _, _)
        | IrOp::CmpGt(d, _, _)
        | IrOp::CmpLtEq(d, _, _)
        | IrOp::CmpGtEq(d, _, _)
        | IrOp::ArrayLoad(d, _, _) => Some(*d),
        IrOp::Call(dest, _, _) => *dest,
        IrOp::StoreVar(_, _)
        | IrOp::ArrayStore(_, _, _)
        | IrOp::DrawSprite { .. }
        | IrOp::ReadInput
        | IrOp::WaitFrame
        | IrOp::Transition(_)
        | IrOp::SourceLoc(_) => None,
    }
}

/// Remove basic blocks that have no incoming edges (except the entry block,
/// which is always reachable by definition).
fn remove_unreachable_blocks(func: &mut IrFunction) {
    if func.blocks.is_empty() {
        return;
    }

    let entry_label = func.blocks[0].label.clone();

    // Collect all labels that are jump/branch targets.
    let mut reachable_labels: HashSet<String> = HashSet::new();
    reachable_labels.insert(entry_label);

    for block in &func.blocks {
        match &block.terminator {
            IrTerminator::Jump(lbl) => {
                reachable_labels.insert(lbl.clone());
            }
            IrTerminator::Branch(_, t, f) => {
                reachable_labels.insert(t.clone());
                reachable_labels.insert(f.clone());
            }
            IrTerminator::Return(_) | IrTerminator::Unreachable => {}
        }
    }

    func.blocks.retain(|b| reachable_labels.contains(&b.label));
}
