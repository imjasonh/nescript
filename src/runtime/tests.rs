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

#[test]
fn multiply_routine_assembles() {
    let mul = gen_multiply();
    // Should have a reasonable number of instructions
    assert!(
        mul.len() > 5,
        "multiply routine too short: {} instructions",
        mul.len()
    );
    let result = asm::assemble(&mul, 0x8000);
    assert!(
        !result.bytes.is_empty(),
        "multiply routine should produce bytes"
    );
    // Should be under 100 bytes (compact 6502 routine)
    assert!(
        result.bytes.len() < 100,
        "multiply routine is {} bytes, expected < 100",
        result.bytes.len()
    );
    // Should contain the __multiply label
    assert!(
        result.labels.contains_key("__multiply"),
        "should define __multiply label"
    );
    // Should end with RTS
    assert_eq!(
        mul.last().unwrap().opcode,
        RTS,
        "multiply routine should end with RTS"
    );
}

// ── Audio driver tests ──

#[test]
fn audio_tick_defines_required_labels() {
    let tick = gen_audio_tick();
    // The IR codegen JSRs into `__audio_tick`; that's the entry.
    let has_entry = tick
        .iter()
        .any(|i| matches!(&i.mode, AM::Label(n) if n == "__audio_tick"));
    assert!(has_entry, "audio tick must define __audio_tick entry label");
    // The tick references `__period_table` via SymbolLo/SymbolHi —
    // the period table itself is linked in separately.
    let refs_period = tick.iter().any(|i| {
        matches!(&i.mode, AM::SymbolLo(n) if n == "__period_table")
            || matches!(&i.mode, AM::SymbolHi(n) if n == "__period_table")
    });
    assert!(
        refs_period,
        "audio tick must reference the __period_table label"
    );
}

#[test]
fn audio_tick_ends_with_rts() {
    let tick = gen_audio_tick();
    assert_eq!(
        tick.last().unwrap().opcode,
        RTS,
        "audio tick must return to caller"
    );
}

#[test]
fn audio_tick_reads_sfx_envelope_via_indirect_y() {
    // The sfx branch walks the envelope via (ZP_SFX_PTR_LO),Y with
    // Y=0 — each NMI reads one byte through the pointer and writes
    // it to $4000. Verify the indirect-indexed load is present.
    let tick = gen_audio_tick();
    let has_load = tick
        .iter()
        .any(|i| i.opcode == LDA && i.mode == AM::IndirectY(ZP_SFX_PTR_LO));
    assert!(
        has_load,
        "audio tick must read envelope via (ZP_SFX_PTR_LO),Y"
    );
}

#[test]
fn audio_tick_writes_pulse1_envelope_register() {
    // After reading the envelope byte the tick writes it to $4000.
    let tick = gen_audio_tick();
    let has_store = tick
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4000));
    assert!(has_store, "audio tick must write pulse-1 envelope to $4000");
}

#[test]
fn audio_tick_mutes_pulse2_on_non_looping_end_of_track() {
    // When a non-looping track hits the (0xFF, 0xFF) sentinel, the
    // tick writes $30 to $4004 and clears ZP_MUSIC_STATE. We verify
    // the mute path exists by checking both writes exist somewhere
    // in the tick body.
    let tick = gen_audio_tick();
    let has_mute = tick
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4004));
    assert!(has_mute, "audio tick must mute pulse-2 on end-of-track");
    let has_state_clear = tick
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(ZP_MUSIC_STATE));
    assert!(
        has_state_clear,
        "audio tick must clear ZP_MUSIC_STATE on stop"
    );
}

#[test]
fn audio_tick_assembles_without_error() {
    // Splice the period table into the same assembly pass so the
    // tick's SymbolLo/SymbolHi references resolve. The tick also
    // uses label-relative branches internally which need to fit
    // within ±127 bytes — if the body grows past that the branches
    // will panic at assemble time.
    let mut combined = gen_audio_tick();
    combined.extend(gen_period_table());
    let result = asm::assemble(&combined, 0xC000);
    assert!(
        !result.bytes.is_empty(),
        "audio tick + period table should assemble"
    );
    assert!(
        result.labels.contains_key("__audio_tick"),
        "audio tick entry label should be exported"
    );
    assert!(
        result.labels.contains_key("__period_table"),
        "period table label should be exported"
    );
}

#[test]
fn period_table_has_60_entries_of_2_bytes() {
    // The table covers C1..B5 inclusive = 60 semitones, 2 bytes
    // each for period_lo and period_hi. Total = 120 data bytes
    // plus the leading label pseudo-instruction.
    let table = gen_period_table();
    // Count the raw bytes in the single `Bytes` block.
    let total: usize = table
        .iter()
        .filter_map(|i| match &i.mode {
            AM::Bytes(v) => Some(v.len()),
            _ => None,
        })
        .sum();
    assert_eq!(total, 120, "period table should be 60 entries × 2 bytes");
}

#[test]
fn period_table_high_bytes_include_length_counter_bit() {
    // Every period_hi byte must have bit 3 set ($08) so the length
    // counter holds the note indefinitely. Without that bit, pulse
    // 2 would silence after a few frames.
    let table = gen_period_table();
    let bytes: Vec<u8> = table
        .iter()
        .filter_map(|i| match &i.mode {
            AM::Bytes(v) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    for (i, chunk) in bytes.chunks(2).enumerate() {
        let hi = chunk[1];
        assert!(
            hi & 0x08 != 0,
            "period table entry {i} high byte ${hi:02X} missing length-counter bit"
        );
    }
}

#[test]
fn period_table_a4_matches_440hz() {
    // Entry for A4 should produce ~253 period. Sanity check the
    // rounding: CPU/(16*440)-1 ≈ 253.12.
    let table = gen_period_table();
    let bytes: Vec<u8> = table
        .iter()
        .filter_map(|i| match &i.mode {
            AM::Bytes(v) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    // A4 is semitone 69 in MIDI. C1 is MIDI 24 (entry 0 in the
    // table). A4 = entry 69 - 24 = 45. Each entry is 2 bytes.
    let lo = bytes[45 * 2];
    let hi = bytes[45 * 2 + 1] & 0x07; // strip length-counter bit
    let period = u16::from_le_bytes([lo, hi]);
    // Expect period ≈ 253 (±1 for rounding).
    assert!(
        (252..=254).contains(&period),
        "A4 period {period} should be ~253"
    );
}

#[test]
fn gen_data_block_emits_label_and_bytes() {
    let block = gen_data_block("__sfx_test", vec![0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(block.len(), 2);
    assert!(matches!(&block[0].mode, AM::Label(n) if n == "__sfx_test"));
    match &block[1].mode {
        AM::Bytes(v) => assert_eq!(v, &[0xDE, 0xAD, 0xBE, 0xEF]),
        other => panic!("expected Bytes, got {other:?}"),
    }
}

#[test]
fn data_block_assembles_verbatim() {
    // A labelled data block must emit exactly the payload bytes
    // (no opcode prefix) and register the label at the payload's
    // address. Verifies the `NOP+Bytes` pseudo doesn't accidentally
    // get wrapped with an instruction byte.
    let block = gen_data_block("__test", vec![0x11, 0x22, 0x33]);
    let result = asm::assemble(&block, 0x8000);
    assert_eq!(result.bytes, vec![0x11, 0x22, 0x33]);
    assert_eq!(result.labels.get("__test").copied(), Some(0x8000));
}

#[test]
fn divide_routine_assembles() {
    let div = gen_divide();
    // Should have a reasonable number of instructions
    assert!(
        div.len() > 5,
        "divide routine too short: {} instructions",
        div.len()
    );
    let result = asm::assemble(&div, 0x8000);
    assert!(
        !result.bytes.is_empty(),
        "divide routine should produce bytes"
    );
    // Should be under 100 bytes (compact 6502 routine)
    assert!(
        result.bytes.len() < 100,
        "divide routine is {} bytes, expected < 100",
        result.bytes.len()
    );
    // Should contain the __divide label
    assert!(
        result.labels.contains_key("__divide"),
        "should define __divide label"
    );
    // Should end with RTS
    assert_eq!(
        div.last().unwrap().opcode,
        RTS,
        "divide routine should end with RTS"
    );
}
