/// Pattern matching subsystem for ROM fingerprinting and data extraction.
///
/// This module provides functions to:
/// - Fingerprint NEScript-produced ROMs (check runtime signatures)
/// - Match FamiTone2 drivers (M4)
/// - Recognize data patterns (palettes, nametables, audio tables) (M3-M4)
pub mod nes_runtime;

/// Fingerprint a set of PRG banks to detect if they were produced by NEScript.
///
/// Returns Some(version_string) if NEScript runtime is detected, None otherwise.
/// M3 MVP: always returns None (stub implementation).
pub fn fingerprint_nescript(_prg_banks: &[Vec<u8>]) -> Option<String> {
    // TODO: Implement NEScript runtime detection in patterns/nes_runtime.rs
    // For now, return None to indicate identity pass-through mode.
    None
}
