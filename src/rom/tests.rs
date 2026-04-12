use super::*;
use crate::parser::ast::Mirroring;

#[test]
fn build_minimal_rom() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_prg(vec![0xEA; 100]); // 100 bytes of NOP
    let rom = builder.build();

    // Should have 16-byte header + 16 KB PRG
    assert_eq!(rom.len(), 16 + 16384);

    // Magic bytes
    assert_eq!(&rom[0..4], b"NES\x1a");

    // 1 PRG bank
    assert_eq!(rom[4], 1);

    // 0 CHR banks (CHR RAM)
    assert_eq!(rom[5], 0);
}

#[test]
fn build_rom_with_chr() {
    let mut builder = RomBuilder::new(Mirroring::Vertical);
    builder.set_prg(vec![0xEA; 100]);
    builder.set_chr(vec![0x00; 16]); // 1 tile
    let rom = builder.build();

    // 16-byte header + 16 KB PRG + 8 KB CHR
    assert_eq!(rom.len(), 16 + 16384 + 8192);
    assert_eq!(rom[5], 1); // 1 CHR bank
}

#[test]
fn horizontal_mirroring() {
    let builder = RomBuilder::new(Mirroring::Horizontal);
    let rom = builder.build();
    assert_eq!(rom[6] & 1, 0);
}

#[test]
fn vertical_mirroring() {
    let builder = RomBuilder::new(Mirroring::Vertical);
    let rom = builder.build();
    assert_eq!(rom[6] & 1, 1);
}

#[test]
fn prg_data_is_padded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_prg(vec![0xAA; 10]);
    let rom = builder.build();

    // First 10 bytes of PRG should be $AA
    assert_eq!(&rom[16..26], &[0xAA; 10]);
    // Rest should be $FF (fill byte)
    assert_eq!(rom[26], 0xFF);
}

#[test]
fn validate_valid_rom() {
    let builder = RomBuilder::new(Mirroring::Horizontal);
    let rom = builder.build();
    let info = validate_ines(&rom).unwrap();
    assert_eq!(info.prg_banks, 1);
    assert_eq!(info.chr_banks, 0);
    assert_eq!(info.mapper, 0);
    assert_eq!(info.mirroring, Mirroring::Horizontal);
}

#[test]
fn validate_bad_magic() {
    let rom = vec![0x00; 100];
    assert!(validate_ines(&rom).is_err());
}

#[test]
fn validate_too_small() {
    let rom = vec![0x4E, 0x45, 0x53];
    assert!(validate_ines(&rom).is_err());
}

#[test]
fn large_prg_uses_two_banks() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_prg(vec![0xEA; 20000]); // > 16 KB
    let rom = builder.build();
    assert_eq!(rom[4], 2); // 2 PRG banks
    assert_eq!(rom.len(), 16 + 32768);
}

#[test]
fn mapper_number_encoded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_mapper(1); // MMC1
    let rom = builder.build();
    let info = validate_ines(&rom).unwrap();
    assert_eq!(info.mapper, 1);
}

#[test]
fn chr_data_is_padded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_chr(vec![0xBB; 16]); // 1 tile = 16 bytes
    let rom = builder.build();

    let chr_start = 16 + 16384; // after header + PRG
    assert_eq!(&rom[chr_start..chr_start + 16], &[0xBB; 16]);
    assert_eq!(rom[chr_start + 16], 0x00); // padded with zeros
}

#[test]
fn mapper_mmc1_encoded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_mapper(crate::rom::mapper_number(crate::parser::ast::Mapper::MMC1));
    let rom = builder.build();
    let info = validate_ines(&rom).unwrap();
    assert_eq!(info.mapper, 1);
}

#[test]
fn mapper_uxrom_encoded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_mapper(crate::rom::mapper_number(crate::parser::ast::Mapper::UxROM));
    let rom = builder.build();
    let info = validate_ines(&rom).unwrap();
    assert_eq!(info.mapper, 2);
}

#[test]
fn mapper_mmc3_encoded() {
    let mut builder = RomBuilder::new(Mirroring::Horizontal);
    builder.set_mapper(crate::rom::mapper_number(crate::parser::ast::Mapper::MMC3));
    let rom = builder.build();
    let info = validate_ines(&rom).unwrap();
    assert_eq!(info.mapper, 4);
}
