use super::*;
use crate::asm;
use crate::asm::{AddressingMode as AM, Opcode::*};

#[test]
fn init_disables_irq() {
    let init = gen_init();
    assert_eq!(init[0].opcode, SEI);
}

#[test]
fn init_sets_stack_pointer() {
    let init = gen_init();
    // LDX #$FF, TXS
    let has_ldx = init
        .iter()
        .any(|i| i.opcode == LDX && i.mode == AM::Immediate(0xFF));
    let has_txs = init.iter().any(|i| i.opcode == TXS);
    assert!(has_ldx, "should load $FF into X");
    assert!(has_txs, "should transfer X to stack pointer");
}

#[test]
fn init_disables_ppu() {
    let init = gen_init();
    // Should write 0 to $2000 and $2001
    let writes_ppu_ctrl = init
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x2000));
    let writes_ppu_mask = init
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x2001));
    assert!(writes_ppu_ctrl, "should disable PPU control");
    assert!(writes_ppu_mask, "should disable PPU mask");
}

#[test]
fn init_enables_nmi_at_end() {
    let init = gen_init();
    // Last STA $2000 should enable NMI (bit 7 set = 0x80)
    let nmi_writes: Vec<_> = init
        .iter()
        .enumerate()
        .filter(|(_, i)| i.opcode == STA && i.mode == AM::Absolute(0x2000))
        .collect();
    assert!(
        nmi_writes.len() >= 2,
        "should write to PPU_CTRL at least twice"
    );
    // The last write should be preceded by LDA #$80
    let last_write_idx = nmi_writes.last().unwrap().0;
    assert!(last_write_idx > 0);
    assert_eq!(init[last_write_idx - 1].opcode, LDA);
    assert_eq!(init[last_write_idx - 1].mode, AM::Immediate(0x80));
}

#[test]
fn init_assembles_without_error() {
    let init = gen_init();
    let result = asm::assemble(&init, 0x8000);
    // Should produce non-empty output
    assert!(!result.bytes.is_empty(), "init should produce bytes");
    // Should be under 200 bytes (the plan estimates ~80)
    assert!(
        result.bytes.len() < 200,
        "init is {} bytes, expected < 200",
        result.bytes.len()
    );
}

#[test]
fn nmi_saves_and_restores_registers() {
    let nmi = gen_nmi();
    // First three instructions should push A, X, Y
    assert_eq!(nmi[0].opcode, PHA);
    assert_eq!(nmi[1].opcode, TXA);
    assert_eq!(nmi[2].opcode, PHA);
    assert_eq!(nmi[3].opcode, TYA);
    assert_eq!(nmi[4].opcode, PHA);

    // Last instructions should restore and RTI
    let len = nmi.len();
    assert_eq!(nmi[len - 1].opcode, RTI);
    assert_eq!(nmi[len - 2].opcode, PLA);
}

#[test]
fn nmi_triggers_oam_dma() {
    let nmi = gen_nmi();
    let has_dma = nmi
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4014));
    assert!(has_dma, "NMI should trigger OAM DMA");
}

#[test]
fn nmi_reads_controller() {
    let nmi = gen_nmi();
    // Should write strobe to $4016
    let has_strobe = nmi
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4016));
    assert!(has_strobe, "NMI should strobe controller");
}

#[test]
fn nmi_sets_frame_flag() {
    let nmi = gen_nmi();
    let has_flag = nmi
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(ZP_FRAME_FLAG));
    assert!(has_flag, "NMI should set frame-ready flag");
}

#[test]
fn nmi_assembles_without_error() {
    let nmi = gen_nmi();
    let result = asm::assemble(&nmi, 0xF000);
    assert!(!result.bytes.is_empty());
    assert!(
        result.bytes.len() < 150,
        "NMI handler is {} bytes, expected < 150",
        result.bytes.len()
    );
}

#[test]
fn irq_handler_is_just_rti() {
    let irq = gen_irq();
    assert_eq!(irq.len(), 1);
    assert_eq!(irq[0].opcode, RTI);
}
