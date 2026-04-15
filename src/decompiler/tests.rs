/// Tests for the decompiler module.

#[test]
fn test_decompile_valid_ines() {
    // Create a minimal valid iNES ROM (header only, minimal PRG/CHR).
    let mut rom_bytes = vec![
        // iNES header
        0x4E, 0x45, 0x53, 0x1A, // Magic: "NES\x1A"
        0x01, // PRG banks: 1 (16 KB)
        0x01, // CHR banks: 1 (8 KB)
        0x00, // Flags 6: horizontal mirroring, mapper 0 (NROM)
        0x00, // Flags 7: mapper 0, iNES 1.0
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Padding
    ];

    // Add minimal PRG bank (16 KB of 0xFF).
    rom_bytes.extend_from_slice(&[0xFF; 16384]);

    // Add minimal CHR bank (8 KB of 0x00).
    rom_bytes.extend_from_slice(&[0x00; 8192]);

    let result = super::decompile_bytes(&rom_bytes);
    assert!(result.is_ok(), "decompile should succeed on valid iNES");

    let decomp = result.unwrap();
    assert_eq!(decomp.rom_info.prg_banks, 1);
    assert_eq!(decomp.rom_info.chr_banks, 1);
    assert_eq!(decomp.mapper, crate::parser::ast::Mapper::NROM);
    assert_eq!(decomp.prg_banks.len(), 1);
    assert_eq!(decomp.prg_banks[0].len(), 16384);
    assert!(decomp.chr_data.is_some());
}

#[test]
fn test_decompile_invalid_magic() {
    let bad_rom = vec![
        0x42, 0x41, 0x44, 0x21, // Bad magic
        0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    let result = super::decompile_bytes(&bad_rom);
    assert!(result.is_err(), "decompile should reject bad magic");
}

#[test]
fn test_decompile_too_small() {
    let tiny_rom = vec![0x4E, 0x45, 0x53]; // Only 3 bytes, need at least 16.

    let result = super::decompile_bytes(&tiny_rom);
    assert!(result.is_err(), "decompile should reject undersized ROM");
}
