pub mod emitter;
pub mod lifter;
/// Decompiler module: converts iNES ROMs back to NEScript .ne source.
///
/// The decompiler pipeline:
/// 1. Fingerprint: Identify if the ROM was produced by NEScript (check runtime signatures)
/// 2. Lift: Extract assets (CHR, palette, nametable data)
/// 3. Emit: Generate .ne source with raw_bank declarations and structured data
///
/// Identity round-trip: ROM → decompile → .ne → compile → ROM (byte-identical)
pub mod patterns;

#[cfg(test)]
mod tests;

use crate::parser::ast::Mapper;
use crate::rom::{validate_ines, RomInfo};
use std::path::Path;

/// Result type for decompiler operations.
pub type DecompilerResult<T> = Result<T, DecompilerError>;

/// Errors that can occur during decompilation.
#[derive(Debug, Clone)]
pub struct DecompilerError {
    pub message: String,
}

impl DecompilerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DecompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DecompilerError {}

/// Information extracted from a decompiled ROM.
#[derive(Debug, Clone)]
pub struct DecompiledRom {
    /// ROM metadata from the iNES header.
    pub rom_info: RomInfo,
    /// Mapper enum (easier to work with than raw number).
    pub mapper: Mapper,
    /// Detected NEScript runtime version (if any); None for unknown/raw ROMs.
    pub nescript_version: Option<String>,
    /// PRG ROM banks as raw binary (ready to emit as raw_bank declarations).
    pub prg_banks: Vec<Vec<u8>>,
    /// CHR ROM bank data (if present).
    pub chr_data: Option<Vec<u8>>,
    /// Extracted palettes (if found).
    pub palettes: Vec<ExtractedPalette>,
    /// Extracted nametables/backgrounds (if found).
    pub backgrounds: Vec<ExtractedBackground>,
}

/// A palette extracted from the ROM.
#[derive(Debug, Clone)]
pub struct ExtractedPalette {
    pub name: String,
    pub colors: [u8; 32],
}

/// A nametable/background extracted from the ROM.
#[derive(Debug, Clone)]
pub struct ExtractedBackground {
    pub name: String,
    pub tiles: [u8; 960],
    pub attributes: [u8; 64],
}

/// Convert an iNES mapper number to a Mapper enum.
fn mapper_from_number(num: u8) -> Mapper {
    match num {
        0 => Mapper::NROM,
        1 => Mapper::MMC1,
        2 => Mapper::UxROM,
        4 => Mapper::MMC3,
        _ => Mapper::NROM, // Default to NROM for unknown mappers
    }
}

/// Decompile an iNES ROM file.
///
/// Reads the ROM from disk, validates it, fingerprints for NEScript runtime,
/// and extracts assets. Returns a `DecompiledRom` containing structured data
/// ready to be emitted as .ne source.
pub fn decompile_rom(rom_path: &Path) -> DecompilerResult<DecompiledRom> {
    let rom_bytes = std::fs::read(rom_path)
        .map_err(|e| DecompilerError::new(format!("failed to read ROM file: {e}")))?;

    decompile_bytes(&rom_bytes)
}

/// Decompile ROM bytes directly (useful for testing).
pub fn decompile_bytes(rom_bytes: &[u8]) -> DecompilerResult<DecompiledRom> {
    let rom_info = validate_ines(rom_bytes)
        .map_err(|e| DecompilerError::new(format!("invalid iNES ROM: {e}")))?;

    let mapper = mapper_from_number(rom_info.mapper);

    // Extract PRG banks.
    let prg_start = 16; // iNES header is 16 bytes.
    let prg_bank_size = 16384;
    let mut prg_banks = Vec::new();
    for i in 0..rom_info.prg_banks {
        let start = prg_start + i * prg_bank_size;
        let end = start + prg_bank_size;
        if end <= rom_bytes.len() {
            prg_banks.push(rom_bytes[start..end].to_vec());
        }
    }

    // Extract CHR data if present.
    let chr_data = if rom_info.chr_banks > 0 {
        let chr_start = prg_start + rom_info.prg_banks * prg_bank_size;
        let chr_size = rom_info.chr_banks * 8192;
        let chr_end = chr_start + chr_size;
        if chr_end <= rom_bytes.len() {
            Some(rom_bytes[chr_start..chr_end].to_vec())
        } else {
            None
        }
    } else {
        None
    };

    // Fingerprint for NEScript runtime (M3 MVP: stub, always returns None).
    let nescript_version = patterns::fingerprint_nescript(&prg_banks);

    // For M3 MVP, skip asset lifting (will be added in later phases).
    // Just return identity pass-through structure.
    let palettes = Vec::new();
    let backgrounds = Vec::new();

    Ok(DecompiledRom {
        rom_info,
        mapper,
        nescript_version,
        prg_banks,
        chr_data,
        palettes,
        backgrounds,
    })
}
