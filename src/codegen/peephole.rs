//! Peephole optimizations over the 6502 instruction stream.
//!
//! Runs after codegen but before assembly, so we can rewrite
//! `Instruction`s directly. Kept conservative to avoid breaking the
//! IR codegen's zero-page slot assumptions.

use crate::asm::{AddressingMode, Instruction, Opcode};

/// Run all peephole passes until fixed point.
pub fn optimize(instructions: &mut Vec<Instruction>) {
    loop {
        let before = instructions.len();
        remove_sta_then_lda(instructions);
        remove_lda_then_sta_same(instructions);
        remove_dead_temp_stores(instructions);
        if instructions.len() == before {
            break;
        }
    }
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
    // ahead to see if the slot is read before being either
    // overwritten or invalidated by a control-flow boundary.
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
        // Scan forward until the slot is read, overwritten, or we hit
        // a control-flow boundary that might branch to code we can't
        // see.
        let mut dead = false;
        for next in instructions.iter().skip(i + 1) {
            if instruction_crosses_block(next) {
                // The slot might be read later; be conservative.
                break;
            }
            if reads_zero_page(next, slot) {
                // A subsequent instruction reads from the slot, so
                // the STA is live.
                break;
            }
            if writes_zero_page(next, slot) {
                // The slot is overwritten with no read in between —
                // the original STA is dead.
                dead = true;
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
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x80)),
            Instruction::new(CLC, AM::Implied),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].opcode, STA);
        assert_eq!(insts[1].opcode, CLC);
    }

    #[test]
    fn keeps_sta_then_lda_user_var() {
        // $10 is a user variable, not a temp slot — must not eliminate.
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(LDA, AM::ZeroPage(0x10)),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
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
    fn preserves_different_addresses() {
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x81)),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
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
