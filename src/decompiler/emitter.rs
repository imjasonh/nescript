/// NEScript source code generation from decompiled data.
///
/// Emits a valid .ne file that round-trips through the compiler back to
/// a byte-identical ROM. The output uses raw_bank declarations for code
/// and structured declarations (palette, background, sfx, music) for assets.
use crate::decompiler::{DecompiledRom, DecompilerError, DecompilerResult};
use crate::parser::ast::Mapper;
use std::path::Path;

/// Emit a decompiled ROM as .ne source code.
pub fn emit_source(rom: &DecompiledRom, output_path: &Path) -> DecompilerResult<()> {
    let source = generate_source(rom)?;
    std::fs::write(output_path, source)
        .map_err(|e| DecompilerError::new(format!("failed to write .ne file: {e}")))?;
    Ok(())
}

/// Generate .ne source as a string (without writing to disk).
pub fn generate_source(rom: &DecompiledRom) -> DecompilerResult<String> {
    let mut output = String::new();

    // Emit game declaration.
    let mapper_name = mapper_name(&rom.mapper);
    let mirroring_name = mirroring_name(rom.rom_info.mirroring);

    output.push_str("game \"DecompiledROM\" {\n");
    output.push_str(&format!("    mapper: {},\n", mapper_name));
    output.push_str(&format!("    mirroring: {},\n", mirroring_name));
    output.push_str("}\n\n");

    // Emit PRG banks as raw_bank declarations.
    for (i, _bank) in rom.prg_banks.iter().enumerate() {
        output.push_str(&format!("raw_bank Bank{} @ {} {{\n", i, i));
        output.push_str(&format!("    binary: \"original.prg.{}.bin\"\n", i));
        output.push_str("}\n\n");
    }

    // Emit CHR if present.
    if let Some(chr_data) = &rom.chr_data {
        if !chr_data.is_empty() {
            output.push_str("raw_bank CHR @ 0 {\n");
            output.push_str("    binary: \"original.chr.bin\"\n");
            output.push_str("}\n\n");
        }
    }

    // Emit extracted palettes (none for M3 MVP).
    for palette in &rom.palettes {
        emit_palette(&mut output, &palette.name, &palette.colors)?;
    }

    // Emit extracted backgrounds (none for M3 MVP).
    for bg in &rom.backgrounds {
        emit_background(&mut output, &bg.name, &bg.tiles, &bg.attributes)?;
    }

    // TODO: Emit extracted audio data (M4).
    // Once audio extraction is implemented in M4, extract SFX and music here.

    Ok(output)
}

/// Emit a palette declaration.
fn emit_palette(output: &mut String, name: &str, colors: &[u8; 32]) -> DecompilerResult<()> {
    output.push_str(&format!("palette {} {{\n", name));
    output.push_str("    colors: [");
    for (i, color) in colors.iter().enumerate() {
        if i > 0 {
            output.push_str(", ");
        }
        if i % 8 == 0 && i > 0 {
            output.push_str("\n    ");
        }
        output.push_str(&format!("0x{:02X}", color));
    }
    output.push_str("]\n");
    output.push_str("}\n\n");
    Ok(())
}

/// Emit a background declaration.
fn emit_background(
    output: &mut String,
    name: &str,
    tiles: &[u8; 960],
    attributes: &[u8; 64],
) -> DecompilerResult<()> {
    output.push_str(&format!("background {} {{\n", name));
    output.push_str("    tiles: [");
    for (i, tile) in tiles.iter().enumerate() {
        if i > 0 {
            output.push_str(", ");
        }
        if i % 16 == 0 && i > 0 {
            output.push_str("\n    ");
        }
        output.push_str(&format!("0x{:02X}", tile));
    }
    output.push_str("]\n");
    output.push_str("    attributes: [");
    for (i, attr) in attributes.iter().enumerate() {
        if i > 0 {
            output.push_str(", ");
        }
        if i % 16 == 0 && i > 0 {
            output.push_str("\n    ");
        }
        output.push_str(&format!("0x{:02X}", attr));
    }
    output.push_str("]\n");
    output.push_str("}\n\n");
    Ok(())
}

/// Convert a Mapper enum to a string name.
fn mapper_name(mapper: &Mapper) -> &'static str {
    match mapper {
        Mapper::NROM => "NROM",
        Mapper::MMC1 => "MMC1",
        Mapper::UxROM => "UxROM",
        Mapper::MMC3 => "MMC3",
    }
}

/// Convert a Mirroring enum to a string name.
fn mirroring_name(mirroring: crate::parser::ast::Mirroring) -> &'static str {
    match mirroring {
        crate::parser::ast::Mirroring::Horizontal => "horizontal",
        crate::parser::ast::Mirroring::Vertical => "vertical",
    }
}

// TODO: Implement SFX and music emission in M4.
