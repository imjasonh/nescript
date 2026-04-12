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
