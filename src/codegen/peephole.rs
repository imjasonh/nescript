//! Peephole optimizations over the 6502 instruction stream.
//!
//! Runs after codegen but before assembly, so we can rewrite
//! `Instruction`s directly. Kept conservative to avoid breaking the
//! IR codegen's zero-page slot assumptions.

use crate::asm::{AddressingMode, Instruction, Opcode};

/// Run all peephole passes until fixed point.
pub fn optimize(instructions: &mut Vec<Instruction>) {
    loop {
        let before_len = instructions.len();
        let before = snapshot(instructions);
        copy_propagate_temps(instructions);
        remove_dead_loads(instructions);
        remove_sta_then_lda(instructions);
        remove_lda_then_sta_same(instructions);
        remove_dead_temp_stores(instructions);
        remove_redundant_loads(instructions);
        // Stop when no pass removed an instruction *and* the stream
        // is unchanged. Copy propagation doesn't shrink the stream —
        // it rewrites operands — so we need the content check too.
        if instructions.len() == before_len && !changed(&before, instructions) {
            break;
        }
    }
}

/// Remove `LDA …` instructions whose value is never read — the next
/// instruction overwrites A without using the current value.
///
/// The heuristic looks one instruction ahead. If the next instruction
/// is an A-clobbering load (`LDA`, `LDX`, `LDY`, `PLA`, `TXA`, `TYA`),
/// the preceding `LDA` is dead. Shifts and arithmetic ops read A, so
/// they don't qualify. A label or branch in between stops the scan
/// (we can't prove A's dead across control flow).
fn remove_dead_loads(instructions: &mut Vec<Instruction>) {
    let mut keep = vec![true; instructions.len()];
    for i in 0..instructions.len() {
        let inst = &instructions[i];
        if inst.opcode != Opcode::LDA {
            continue;
        }
        // Find the next instruction that isn't a label definition.
        let mut j = i + 1;
        while j < instructions.len() {
            let next = &instructions[j];
            // Label definitions are passive markers; skip over them.
            if next.opcode == Opcode::NOP && matches!(next.mode, AddressingMode::Label(_)) {
                j += 1;
                continue;
            }
            break;
        }
        if j >= instructions.len() {
            continue;
        }
        let next = &instructions[j];
        if matches!(
            next.opcode,
            Opcode::LDA | Opcode::PLA | Opcode::TXA | Opcode::TYA
        ) {
            // A is about to be overwritten without being used.
            keep[i] = false;
        }
    }
    let mut out = Vec::with_capacity(instructions.len());
    for (i, inst) in instructions.iter().enumerate() {
        if keep[i] {
            out.push(inst.clone());
        }
    }
    *instructions = out;
}

fn snapshot(instructions: &[Instruction]) -> Vec<(Opcode, AddressingMode)> {
    instructions
        .iter()
        .map(|i| (i.opcode, i.mode.clone()))
        .collect()
}

fn changed(before: &[(Opcode, AddressingMode)], after: &[Instruction]) -> bool {
    if before.len() != after.len() {
        return true;
    }
    before
        .iter()
        .zip(after.iter())
        .any(|((op, mode), inst)| *op != inst.opcode || *mode != inst.mode)
}

/// Copy propagation for IR temp slots.
///
/// After the IR codegen, each temp produced by an IR op is spilled
/// to a zero-page slot via `STA slot`. Any subsequent op that needs
/// the temp emits `LDA slot`. Copy propagation notes the *source*
/// of each temp store (e.g. "slot 128 was set from [slot 16]") and
/// rewrites subsequent loads of that slot to load from the source
/// directly. If the source is later written, the equivalence is
/// invalidated before we see the consuming load. Immediate values
/// are tracked the same way.
///
/// The rewrite doesn't remove anything — it just swaps the operand.
/// The other peephole passes then pick up the now-dead `STA slot`
/// and drop it.
fn copy_propagate_temps(instructions: &mut [Instruction]) {
    use std::collections::HashMap;

    // slot -> source of the most recent STA into that slot.
    let mut temp_source: HashMap<u8, Source> = HashMap::new();
    // Track what A currently holds so we can decide whether an STA
    // actually copies a known source into the slot.
    let mut a: Option<Source> = None;

    for inst in instructions.iter_mut() {
        if instruction_crosses_block(inst) {
            temp_source.clear();
            a = None;
            continue;
        }
        match (inst.opcode, inst.mode.clone()) {
            (Opcode::LDA, AddressingMode::Immediate(v)) => {
                a = Some(Source::Imm(v));
            }
            (Opcode::LDA, AddressingMode::ZeroPage(addr)) => {
                // If addr is a temp whose source is tracked and the
                // source still holds the intended value, rewrite the
                // load to use the source directly.
                if is_temp_slot_addr(addr) {
                    if let Some(src) = temp_source.get(&addr).copied() {
                        match src {
                            Source::Imm(v) => {
                                inst.mode = AddressingMode::Immediate(v);
                                a = Some(Source::Imm(v));
                                continue;
                            }
                            Source::Zp(orig) => {
                                inst.mode = AddressingMode::ZeroPage(orig);
                                a = Some(Source::Zp(orig));
                                continue;
                            }
                        }
                    }
                }
                a = Some(Source::Zp(addr));
            }
            // Arithmetic / logical ops that read a temp slot: rewrite
            // the operand to the temp's tracked source. Clobbers A.
            (
                Opcode::ADC | Opcode::SBC | Opcode::AND | Opcode::ORA | Opcode::EOR | Opcode::CMP,
                AddressingMode::ZeroPage(addr),
            ) if is_temp_slot_addr(addr) => {
                if let Some(src) = temp_source.get(&addr).copied() {
                    inst.mode = match src {
                        Source::Imm(v) => AddressingMode::Immediate(v),
                        Source::Zp(orig) => AddressingMode::ZeroPage(orig),
                    };
                }
                a = None;
            }
            (Opcode::STA, AddressingMode::ZeroPage(addr)) => {
                // If we're storing to a temp slot and we know the
                // source of the value in A, record the equivalence.
                if is_temp_slot_addr(addr) {
                    if let Some(src) = a {
                        temp_source.insert(addr, src);
                    } else {
                        temp_source.remove(&addr);
                    }
                } else {
                    // Storing to a non-temp ZP addr invalidates any
                    // temp that was tracking [addr] as its source.
                    temp_source.retain(|_, src| !matches!(*src, Source::Zp(a) if a == addr));
                }
            }
            // Any op that reads a non-ZP location or clobbers A: just
            // invalidate A; temps are unaffected.
            _ => {
                if modifies_a(inst.opcode) {
                    a = None;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Imm(u8),
    Zp(u8),
}

fn is_temp_slot_addr(addr: u8) -> bool {
    addr >= 0x80
}

fn modifies_a(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::LDA
            | Opcode::ADC
            | Opcode::SBC
            | Opcode::AND
            | Opcode::ORA
            | Opcode::EOR
            | Opcode::ASL
            | Opcode::LSR
            | Opcode::ROL
            | Opcode::ROR
            | Opcode::PLA
            | Opcode::TXA
            | Opcode::TYA
    )
}

/// Track what `A` holds through a linear run of instructions and
/// eliminate `LDA` that would reload a value A already has.
///
/// The tracker knows about three states:
/// - `AKnown::None` — A's value is unknown
/// - `AKnown::Addr(addr)` — A equals the byte currently at `addr`
/// - `AKnown::Imm(val)` — A equals the immediate value `val`
///
/// After any instruction that may clobber A (ADC, AND, etc.) we
/// transition to `None`. After `STA addr` A is still known and we
/// additionally record that `addr` now equals A's known value (so a
/// later `LDA addr` is redundant). Any control-flow instruction or
/// label resets the tracker.
fn remove_redundant_loads(instructions: &mut Vec<Instruction>) {
    use AKnown::*;
    let mut keep = vec![true; instructions.len()];
    let mut a: AKnown = None;
    for (i, inst) in instructions.iter().enumerate() {
        if instruction_crosses_block(inst) {
            a = None;
            continue;
        }
        match (inst.opcode, &inst.mode) {
            (Opcode::LDA, AddressingMode::Immediate(v)) => {
                if let Imm(existing) = a {
                    if existing == *v {
                        keep[i] = false;
                        continue;
                    }
                }
                a = Imm(*v);
            }
            (Opcode::LDA, AddressingMode::ZeroPage(addr)) => {
                if let Addr(existing) = a {
                    if existing == *addr {
                        keep[i] = false;
                        continue;
                    }
                }
                a = Addr(*addr);
            }
            (Opcode::LDA, AddressingMode::Absolute(addr)) => {
                // We could track absolute addresses too, but don't
                // try to unify them with ZP. Just record the value.
                // Not going to eliminate against prior state.
                let _ = addr;
                a = None;
            }
            (Opcode::STA, AddressingMode::ZeroPage(addr)) => {
                // After STA, A is unchanged. Additionally, `addr` now
                // holds A's value. Remember that equivalence: a later
                // `LDA addr` is redundant.
                a = Addr(*addr);
            }
            (Opcode::STA, _) => {
                // A unchanged, but we don't track non-ZP addresses.
            }
            // Ops that clobber A — clear tracker.
            (
                Opcode::ADC
                | Opcode::SBC
                | Opcode::AND
                | Opcode::ORA
                | Opcode::EOR
                | Opcode::ASL
                | Opcode::LSR
                | Opcode::ROL
                | Opcode::ROR
                | Opcode::LDX
                | Opcode::LDY
                | Opcode::PLA
                | Opcode::TXA
                | Opcode::TYA,
                _,
            ) => {
                a = None;
            }
            // Ops that don't touch A — leave the tracker alone.
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(instructions.len());
    for (i, inst) in instructions.iter().enumerate() {
        if keep[i] {
            out.push(inst.clone());
        }
    }
    *instructions = out;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AKnown {
    None,
    Addr(u8),
    Imm(u8),
}

/// Remove `STA temp_slot` instructions whose written value is never
/// read before the slot is overwritten or we cross a control-flow
/// boundary (label, branch, jump, call, return).
///
/// This targets the IR codegen's pattern where each op spills its
/// result to an IR temp slot even if the next op consumes it by
/// reading directly from that slot — but nothing further does. The
/// final store-to-user-variable covers the actual need; the intermediate
/// store-to-temp is dead.
fn remove_dead_temp_stores(instructions: &mut Vec<Instruction>) {
    // Walk forward. For each `STA slot` where slot is a temp, look
    // ahead through the rest of the current function (bounded by the
    // next `__ir_fn_` label or EOF). If no subsequent instruction
    // reads the slot before the function ends or the slot is
    // overwritten, the STA is dead.
    //
    // IR temp slots are assigned uniquely per IR op within a function
    // (the IR codegen resets `next_temp_slot` to 0 at each function
    // entry and never reuses slot numbers within a function), so it's
    // safe to scan past labels and branches within the same function.
    // Cross-function JSR can't read our temps because the callee also
    // starts its own temp numbering from $80 — if it stomps on our
    // slots, we can't have relied on them surviving the call anyway.
    let mut keep = vec![true; instructions.len()];
    for i in 0..instructions.len() {
        let inst = &instructions[i];
        if inst.opcode != Opcode::STA {
            continue;
        }
        let slot = match inst.mode {
            AddressingMode::ZeroPage(addr) if addr >= 0x80 => addr,
            _ => continue,
        };
        let mut dead = true;
        for next in instructions.iter().skip(i + 1) {
            if is_function_boundary(next) {
                break;
            }
            if reads_zero_page(next, slot) {
                dead = false;
                break;
            }
            if writes_zero_page(next, slot) {
                // Overwritten with no read in between — our STA is
                // dead. Stop scanning either way.
                break;
            }
        }
        if dead {
            keep[i] = false;
        }
    }
    let mut out = Vec::with_capacity(instructions.len());
    for (i, inst) in instructions.iter().enumerate() {
        if keep[i] {
            out.push(inst.clone());
        }
    }
    *instructions = out;
}

/// True if `inst` marks the start of a new function. IR codegen emits
/// function bodies with a `NOP Label("__ir_fn_…")` pseudo-instruction.
/// A JSR to a function also transfers control away, but since each
/// function starts its own temp slot allocation at $80 we don't rely
/// on slot contents surviving calls.
fn is_function_boundary(inst: &Instruction) -> bool {
    if inst.opcode == Opcode::NOP {
        if let AddressingMode::Label(name) = &inst.mode {
            return name.starts_with("__ir_fn_")
                || name.starts_with("__fn_")
                || name == "__reset"
                || name == "__nmi"
                || name == "__irq";
        }
    }
    matches!(inst.opcode, Opcode::RTS | Opcode::RTI)
}

/// True if the given instruction is a control-flow boundary — we can't
/// safely reason about liveness across it.
fn instruction_crosses_block(inst: &Instruction) -> bool {
    // Branches / jumps / calls / returns all count as boundaries
    // because they might transfer to code that reads the slot.
    if matches!(
        inst.opcode,
        Opcode::JMP
            | Opcode::JSR
            | Opcode::RTS
            | Opcode::RTI
            | Opcode::BEQ
            | Opcode::BNE
            | Opcode::BCC
            | Opcode::BCS
            | Opcode::BMI
            | Opcode::BPL
            | Opcode::BVC
            | Opcode::BVS
            | Opcode::BRK
    ) {
        return true;
    }
    // A label definition (NOP with `Label` operand) is also a boundary —
    // it's a potential jump target, and we can't see where jumps come
    // from without a full control-flow graph.
    matches!(inst.mode, AddressingMode::Label(_))
}

/// True if `inst` reads from the given zero-page address.
fn reads_zero_page(inst: &Instruction, addr: u8) -> bool {
    let targets_same = matches!(
        inst.mode,
        AddressingMode::ZeroPage(a) if a == addr
    );
    if !targets_same {
        return false;
    }
    // Reading instructions: LDA/LDX/LDY, arithmetic ops, comparisons,
    // BIT — anything that consumes the byte at the address.
    matches!(
        inst.opcode,
        Opcode::LDA
            | Opcode::LDX
            | Opcode::LDY
            | Opcode::ADC
            | Opcode::SBC
            | Opcode::AND
            | Opcode::ORA
            | Opcode::EOR
            | Opcode::CMP
            | Opcode::CPX
            | Opcode::CPY
            | Opcode::BIT
            | Opcode::ASL
            | Opcode::LSR
            | Opcode::ROL
            | Opcode::ROR
            | Opcode::INC
            | Opcode::DEC
    )
}

/// True if `inst` writes to the given zero-page address (overwriting
/// whatever was there). We treat read-modify-write ops as reads, not
/// writes — they preserve the "was read" bit for the original STA.
fn writes_zero_page(inst: &Instruction, addr: u8) -> bool {
    if !matches!(inst.mode, AddressingMode::ZeroPage(a) if a == addr) {
        return false;
    }
    matches!(inst.opcode, Opcode::STA | Opcode::STX | Opcode::STY)
}

/// Remove `LDA addr` immediately followed by `STA addr` (same addr).
/// The store is a no-op because the byte is already there.
fn remove_lda_then_sta_same(instructions: &mut Vec<Instruction>) {
    let mut out = Vec::with_capacity(instructions.len());
    let mut i = 0;
    while i < instructions.len() {
        if i + 1 < instructions.len() {
            let a = &instructions[i];
            let b = &instructions[i + 1];
            if a.opcode == Opcode::LDA && b.opcode == Opcode::STA && a.mode == b.mode {
                // Keep the LDA (in case the value in A is used later)
                // but drop the pointless STA.
                out.push(a.clone());
                i += 2;
                continue;
            }
        }
        out.push(instructions[i].clone());
        i += 1;
    }
    *instructions = out;
}

/// Remove `STA slot` immediately followed by `LDA slot` when both
/// refer to an IR temp slot. The LDA is redundant because A already
/// holds the value we just stored.
///
/// This targets the IR codegen's store-every-temp pattern: ops that
/// produce a value into `A` then immediately store it, and the next
/// op loads it back.
fn remove_sta_then_lda(instructions: &mut Vec<Instruction>) {
    let mut out = Vec::with_capacity(instructions.len());
    let mut i = 0;
    while i < instructions.len() {
        if i + 1 < instructions.len() {
            let a = &instructions[i];
            let b = &instructions[i + 1];
            if a.opcode == Opcode::STA
                && b.opcode == Opcode::LDA
                && a.mode == b.mode
                && is_temp_slot(&a.mode)
            {
                // Keep the STA (subsequent code may read the slot),
                // drop the LDA.
                out.push(a.clone());
                i += 2;
                continue;
            }
        }
        out.push(instructions[i].clone());
        i += 1;
    }
    *instructions = out;
}

/// True if the addressing mode targets an IR temp slot ($80-$FF).
/// We restrict peephole store/load elimination to temp slots so we
/// don't accidentally merge accesses to user variables in ZP (where
/// an intervening call or IRQ could have clobbered A).
fn is_temp_slot(mode: &AddressingMode) -> bool {
    matches!(mode, AddressingMode::ZeroPage(addr) if *addr >= 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::AddressingMode as AM;
    use crate::asm::Opcode::*;

    #[test]
    fn removes_sta_then_lda_temp() {
        // `STA $80; LDA $80; CLC; RTS` — the STA is dead (the slot
        // is never read by anything reachable), and even if we did
        // keep the STA the LDA would be eliminated by A-tracking
        // since A already holds the value we just stored. Both get
        // stripped, leaving just CLC + RTS.
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x80)),
            Instruction::new(CLC, AM::Implied),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].opcode, CLC);
        assert_eq!(insts[1].opcode, RTS);
    }

    #[test]
    fn keeps_sta_when_temp_is_read_later() {
        // STA $80; LDA #5; ORA $80 — the ORA reads slot $80, so
        // the STA is live.
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::Immediate(5)),
            Instruction::new(ORA, AM::ZeroPage(0x80)),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        // STA must remain because the ORA reads it.
        assert!(
            insts
                .iter()
                .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x80)),
            "STA to $80 should be preserved: {insts:?}"
        );
    }

    #[test]
    fn eliminates_sta_then_lda_via_a_tracking() {
        // Even for user variables, `STA $10; LDA $10` is redundant in
        // straight-line code: A still holds the value we just stored.
        // The A-value tracker handles this. (If a JSR or branch
        // intervenes, the tracker clears and the LDA is preserved.)
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(LDA, AM::ZeroPage(0x10)),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0].opcode, STA);
    }

    #[test]
    fn preserves_lda_across_jsr() {
        // JSR clobbers A (callee can do anything), so the second LDA
        // must survive.
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(JSR, AM::Label("foo".into())),
            Instruction::new(LDA, AM::ZeroPage(0x10)),
        ];
        optimize(&mut insts);
        // STA, JSR, LDA — all preserved
        assert_eq!(insts.len(), 3);
    }

    #[test]
    fn removes_lda_then_sta_same_address() {
        let mut insts = vec![
            Instruction::new(LDA, AM::ZeroPage(0x10)),
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(CLC, AM::Implied),
        ];
        optimize(&mut insts);
        // LDA kept (value in A may be used), pointless STA removed
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].opcode, LDA);
        assert_eq!(insts[1].opcode, CLC);
    }

    #[test]
    fn preserves_sta_when_slot_is_read() {
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x81)),
            Instruction::new(CMP, AM::ZeroPage(0x80)),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        // STA $80 is live because CMP reads it.
        assert!(
            insts
                .iter()
                .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x80)),
            "STA should be preserved when slot is later read: {insts:?}"
        );
    }

    #[test]
    fn copy_propagation_rewrites_temp_load_to_source() {
        // LDA $10; STA $80; LDA $80 should become LDA $10; (stores
        // trimmed by dead-store). End result: just a single LDA.
        let mut insts = vec![
            Instruction::new(LDA, AM::ZeroPage(0x10)),
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x80)),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        // After copy prop + dead store elim + A-tracking, the sequence
        // collapses to a single LDA + RTS.
        assert!(
            insts
                .iter()
                .any(|i| i.opcode == LDA && i.mode == AM::ZeroPage(0x10)),
            "should preserve original LDA: {insts:?}"
        );
    }

    #[test]
    fn copy_propagation_rewrites_arithmetic_operand() {
        // LDA #1; STA $80; LDA $10; CLC; ADC $80 — the ADC should be
        // rewritten to `ADC #1` because $80 is a temp tracked to Imm(1).
        let mut insts = vec![
            Instruction::new(LDA, AM::Immediate(1)),
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x10)),
            Instruction::new(CLC, AM::Implied),
            Instruction::new(ADC, AM::ZeroPage(0x80)),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        assert!(
            insts
                .iter()
                .any(|i| i.opcode == ADC && i.mode == AM::Immediate(1)),
            "ADC should be rewritten to immediate: {insts:?}"
        );
    }

    #[test]
    fn dead_load_elimination_drops_overwritten_lda() {
        // LDA $10; LDA #0 — the first LDA is dead because A is
        // overwritten before being used.
        let mut insts = vec![
            Instruction::new(LDA, AM::ZeroPage(0x10)),
            Instruction::new(LDA, AM::Immediate(0)),
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(RTS, AM::Implied),
        ];
        optimize(&mut insts);
        // First LDA dropped; result is LDA #0, STA $10, RTS
        let lda_count = insts.iter().filter(|i| i.opcode == LDA).count();
        assert_eq!(lda_count, 1, "expected one LDA, got: {insts:?}");
    }

    #[test]
    fn idempotent_on_optimized_code() {
        let mut insts = vec![
            Instruction::new(LDA, AM::Immediate(5)),
            Instruction::new(CLC, AM::Implied),
            Instruction::new(RTS, AM::Implied),
        ];
        let before = insts.len();
        optimize(&mut insts);
        assert_eq!(insts.len(), before);
    }
}
