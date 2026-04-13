use super::*;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::parser::ast::Mirroring;
use crate::rom;

#[test]
fn link_produces_valid_ines() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![
        Instruction::new(NOP, AM::Label("__main_loop".into())),
        Instruction::implied(NOP),
        Instruction::new(JMP, AM::Label("__main_loop".into())),
    ];
    let rom_data = linker.link(&user_code);
    let info = rom::validate_ines(&rom_data).unwrap();
    assert_eq!(info.prg_banks, 1);
    assert_eq!(info.chr_banks, 1);
    assert_eq!(info.mapper, 0);
}

#[test]
fn link_has_correct_vector_table() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];
    let rom_data = linker.link(&user_code);

    // Vector table is at the last 6 bytes of PRG ROM
    // PRG starts at offset 16 in the .nes file
    let prg_end = 16 + 16384;
    let vector_start = prg_end - 6;

    // NMI vector (2 bytes, little-endian)
    let nmi = u16::from_le_bytes([rom_data[vector_start], rom_data[vector_start + 1]]);
    // RESET vector
    let reset = u16::from_le_bytes([rom_data[vector_start + 2], rom_data[vector_start + 3]]);
    // IRQ vector
    let irq = u16::from_le_bytes([rom_data[vector_start + 4], rom_data[vector_start + 5]]);

    // All vectors should be in the $C000-$FFFF range
    assert!(nmi >= 0xC000, "NMI vector {nmi:#06X} should be >= $C000");
    assert!(
        reset >= 0xC000,
        "RESET vector {reset:#06X} should be >= $C000"
    );
    assert!(irq >= 0xC000, "IRQ vector {irq:#06X} should be >= $C000");

    // RESET should point to the start of code ($C000)
    assert_eq!(reset, 0xC000, "RESET should point to $C000");
}

#[test]
fn link_includes_chr_data() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];
    let rom_data = linker.link(&user_code);

    // CHR starts after PRG
    let chr_start = 16 + 16384;
    // First 16 bytes should be the smiley sprite
    assert_ne!(
        &rom_data[chr_start..chr_start + 16],
        &[0u8; 16],
        "CHR data should contain sprite tile"
    );
}

#[test]
fn link_rom_size_correct() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];
    let rom_data = linker.link(&user_code);

    // 16 header + 16384 PRG + 8192 CHR
    assert_eq!(rom_data.len(), 16 + 16384 + 8192);
}

#[test]
fn link_with_sprites_places_chr_data() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];

    let sprite_bytes: Vec<u8> = (0x20..0x30).collect(); // 16 bytes, one tile
    let sprites = vec![SpriteData {
        name: "Player".into(),
        tile_index: 1,
        chr_bytes: sprite_bytes.clone(),
    }];

    let rom_data = linker.link_with_assets(&user_code, &sprites);

    // CHR starts right after the 16-byte iNES header and 16 KB PRG bank.
    let chr_start = 16 + 16384;

    // Tile 0 should still contain the built-in smiley (first 16 bytes, not
    // all zero).
    let tile0 = &rom_data[chr_start..chr_start + 16];
    assert_ne!(
        tile0, &[0u8; 16],
        "default smiley should occupy tile index 0",
    );

    // Tile 1 (CHR offset 16) should contain the sprite's CHR bytes exactly.
    let tile1 = &rom_data[chr_start + 16..chr_start + 32];
    assert_eq!(tile1, sprite_bytes.as_slice());

    // Tile 2 and beyond should be untouched (all zeros).
    let tile2 = &rom_data[chr_start + 32..chr_start + 48];
    assert_eq!(tile2, &[0u8; 16]);
}

#[test]
fn link_with_sprites_spanning_multiple_tiles() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];

    // 32 bytes = 2 tiles. The linker should place them consecutively
    // starting at the requested tile index.
    let sprite_bytes: Vec<u8> = (0..32).collect();
    let sprites = vec![SpriteData {
        name: "Big".into(),
        tile_index: 4,
        chr_bytes: sprite_bytes.clone(),
    }];

    let rom_data = linker.link_with_assets(&user_code, &sprites);
    let chr_start = 16 + 16384;

    // Tile 4 starts at CHR offset 64.
    let placed = &rom_data[chr_start + 64..chr_start + 64 + 32];
    assert_eq!(placed, sprite_bytes.as_slice());
}

#[test]
fn link_splices_audio_tick_when_user_marker_present() {
    // When user code contains the `__audio_used` marker label (the
    // IR codegen emits this whenever it sees a `play`/`start_music`/
    // `stop_music` op), the linker must splice a `JSR __audio_tick`
    // into the NMI handler prologue AND link in the audio driver
    // body so the JSR target exists.
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![
        // Pretend user code with the marker the codegen would emit.
        Instruction::new(NOP, AM::Label("__audio_used".into())),
        Instruction::implied(NOP),
    ];
    let rom_data = linker.link(&user_code);

    // The ROM should be valid even with the splice — the driver
    // body has to fit in bank 0 without overflowing.
    let info = rom::validate_ines(&rom_data).unwrap();
    assert_eq!(info.prg_banks, 1);
}

#[test]
fn link_omits_audio_tick_when_no_marker() {
    // User code without the marker should not pay any ROM cost
    // for the audio driver. We can't easily inspect bytes, but we
    // can at least verify the ROM builds and has a normal shape.
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];
    let rom_data = linker.link(&user_code);
    let info = rom::validate_ines(&rom_data).unwrap();
    assert_eq!(info.prg_banks, 1);
}

#[test]
fn link_with_audio_data_places_sfx_blobs_in_prg() {
    // When user code has the `__audio_used` marker AND we pass in
    // sfx data, the linker must:
    //   1. Splice in the audio tick (driver body)
    //   2. Splice in the period table
    //   3. Splice in the envelope blob under its label
    //   4. Resolve SymbolLo/SymbolHi references from user code to
    //      the blob's address (second pass of the assembler)
    let linker = Linker::new(Mirroring::Horizontal);
    // User code: `LDA #<__sfx_test`, `STA $0C`, `LDA #>__sfx_test`,
    // `STA $0D` — simulates what IR codegen's gen_play_sfx emits.
    let user_code = vec![
        Instruction::new(NOP, AM::Label("__audio_used".into())),
        Instruction::new(LDA, AM::SymbolLo("__sfx_test".into())),
        Instruction::new(STA, AM::ZeroPage(0x0C)),
        Instruction::new(LDA, AM::SymbolHi("__sfx_test".into())),
        Instruction::new(STA, AM::ZeroPage(0x0D)),
    ];
    let sfx = vec![SfxData {
        name: "test".into(),
        period_lo: 0x50,
        period_hi: 0x08,
        envelope: vec![0xBF, 0xB8, 0xB4, 0xB0, 0x00],
    }];
    let rom = linker.link_with_all_assets(&user_code, &[], &sfx, &[]);
    let info = rom::validate_ines(&rom).unwrap();
    assert_eq!(info.prg_banks, 1);
    // The envelope bytes must appear somewhere in PRG. Find them.
    let prg = &rom[16..16 + 16384];
    let needle = [0xBF, 0xB8, 0xB4, 0xB0, 0x00];
    let found = prg.windows(needle.len()).any(|w| w == needle);
    assert!(found, "sfx envelope bytes should be spliced into PRG ROM");
}

#[test]
fn link_with_audio_data_places_music_stream_in_prg() {
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![
        Instruction::new(NOP, AM::Label("__audio_used".into())),
        Instruction::new(LDA, AM::SymbolLo("__music_test".into())),
        Instruction::new(STA, AM::ZeroPage(0x0E)),
        Instruction::new(LDA, AM::SymbolHi("__music_test".into())),
        Instruction::new(STA, AM::ZeroPage(0x0F)),
    ];
    let music = vec![MusicData {
        name: "test".into(),
        header: 0xA9,
        stream: vec![37, 8, 41, 8, 44, 8, 0xFF, 0xFF],
    }];
    let rom = linker.link_with_all_assets(&user_code, &[], &[], &music);
    let prg = &rom[16..16 + 16384];
    let needle = [37, 8, 41, 8, 44, 8, 0xFF, 0xFF];
    let found = prg.windows(needle.len()).any(|w| w == needle);
    assert!(found, "music note stream should be spliced into PRG ROM");
}

#[test]
fn link_with_audio_resolves_sfx_pointer_references() {
    // The SymbolLo/SymbolHi references in user code must get
    // fixed up to the *actual* PRG address of the envelope blob.
    // We can verify this by reading back the user code bytes and
    // checking that the LDA immediates point somewhere in the
    // valid PRG range ($C000-$FFFF).
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![
        Instruction::new(NOP, AM::Label("__audio_used".into())),
        Instruction::new(LDA, AM::SymbolLo("__sfx_test".into())),
        Instruction::new(LDA, AM::SymbolHi("__sfx_test".into())),
    ];
    let sfx = vec![SfxData {
        name: "test".into(),
        period_lo: 0x50,
        period_hi: 0x08,
        envelope: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00],
    }];
    let rom = linker.link_with_all_assets(&user_code, &[], &sfx, &[]);
    // The user code starts at RESET ($C000) after init+palette_load.
    // Rather than compute the exact offset, verify the envelope
    // bytes appear at a byte that matches what the LDA immediate
    // pair would produce. We find the immediate pair by searching
    // for `A9 xx A9 yy` and checking `$xxyy` points at the needle.
    let prg = &rom[16..16 + 16384];
    let needle = [0xDE, 0xAD, 0xBE, 0xEF, 0x00];
    // Find where the envelope lives in ROM.
    let env_offset = prg
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("envelope should be in PRG");
    let env_addr = 0xC000u16 + env_offset as u16;
    // Find any LDA-immediate pair that matches the envelope address.
    let lo = (env_addr & 0xFF) as u8;
    let hi = (env_addr >> 8) as u8;
    let pattern = [0xA9u8, lo, 0xA9, hi];
    let matched = prg.windows(pattern.len()).any(|w| w == pattern);
    assert!(
        matched,
        "should find `LDA #<env_addr; LDA #>env_addr` in PRG ($A9 ${lo:02X} $A9 ${hi:02X})"
    );
}

#[test]
fn link_without_audio_marker_does_not_emit_period_table() {
    // Programs that never use audio must not pay the cost of the
    // period table, driver body, or any blobs. We verify this
    // indirectly: the `__period_table` label should NOT appear at
    // a distinct ROM address from the main body.
    let linker = Linker::new(Mirroring::Horizontal);
    let user_code = vec![Instruction::implied(NOP)];
    let rom = linker.link_with_all_assets(
        &user_code,
        &[],
        &[SfxData {
            name: "unused".into(),
            period_lo: 0,
            period_hi: 0,
            envelope: vec![0xAA, 0xBB, 0x00],
        }],
        &[],
    );
    // The envelope bytes `AA BB 00` must NOT appear in PRG — the
    // linker should have elided the whole audio section because
    // the marker is absent.
    let prg = &rom[16..16 + 16384];
    let needle = [0xAAu8, 0xBB, 0x00];
    let found = prg.windows(needle.len()).any(|w| w == needle);
    assert!(
        !found,
        "unused sfx data should not be spliced when __audio_used is absent"
    );
}

#[test]
fn palette_load_writes_to_ppu() {
    let linker = Linker::new(Mirroring::Horizontal);
    let palette_insts = linker.gen_palette_load();

    // Should write to PPU address register ($2006) twice
    let ppu_addr_writes: Vec<_> = palette_insts
        .iter()
        .filter(|i| i.opcode == STA && i.mode == AM::Absolute(0x2006))
        .collect();
    assert_eq!(
        ppu_addr_writes.len(),
        2,
        "should set PPU address (high and low bytes)"
    );

    // Should write 32 palette bytes to $2007
    let ppu_data_writes: Vec<_> = palette_insts
        .iter()
        .filter(|i| i.opcode == STA && i.mode == AM::Absolute(0x2007))
        .collect();
    assert_eq!(
        ppu_data_writes.len(),
        32,
        "should write all 32 palette bytes"
    );
}
