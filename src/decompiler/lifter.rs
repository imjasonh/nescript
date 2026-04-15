/// Asset lifting: extract CHR, palette, nametable, and audio data from ROMs.
///
/// M3 MVP: Stub implementation (no extraction yet, just identity pass-through).
/// M4+: Will extract data patterns and convert to structured declarations.

/// Extract CHR data from ROM bytes and optionally convert to PNG.
///
/// Returns the raw CHR bytes. Future versions can optionally convert to PNG
/// via an external library (pngjs or custom encoder).
pub fn extract_chr(chr_data: &[u8]) -> Vec<u8> {
    chr_data.to_vec()
}

/// Search for and extract palettes from PRG data.
///
/// Looks for 32-byte palette blocks (common pattern in NEScript ROMs).
/// M3 MVP: Returns empty vec (stub).
pub fn extract_palettes(_prg_banks: &[Vec<u8>]) -> Vec<(String, [u8; 32])> {
    vec![]
}

/// Search for and extract nametables (backgrounds) from PRG data.
///
/// Looks for 960+64 byte blocks (nametable + attributes).
/// M3 MVP: Returns empty vec (stub).
pub fn extract_backgrounds(_prg_banks: &[Vec<u8>]) -> Vec<(String, [u8; 960], [u8; 64])> {
    vec![]
}
