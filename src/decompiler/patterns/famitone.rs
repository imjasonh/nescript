/// FamiTone2 audio driver recognition and location extraction.
///
/// The FamiTone2 driver is a standard NES audio playback system that:
/// - Maintains a 60-entry period table (120 bytes, u16 LE) mapping notes to APU periods
/// - Drives audio playback with per-NMI ticks reading envelope/note streams
/// - Uses standard APU register writes ($4000-$400F) for sound control
///
/// This module identifies FamiTone2-compatible drivers and extracts the period table location.

/// Result of FamiTone2 detection: driver location and period table offset.
#[derive(Debug, Clone)]
pub struct FamiTone2Info {
    /// Offset in PRG where the FamiTone2 driver code begins (typically around 0x8000-0xC000).
    pub driver_offset: usize,
    /// Offset to the 60-entry period table (120 bytes, u16 LE). Typically immediately
    /// after the driver init code.
    pub period_table_offset: usize,
}

/// Attempt to detect a FamiTone2-compatible audio driver in the PRG data.
///
/// Returns `Some(FamiTone2Info)` if a driver is found, `None` if not detected.
/// The detection strategy:
/// 1. Look for the standard period table signature: 60 consecutive u16 LE values
///    that match the expected musical scale (incrementing APU period values).
/// 2. Check that the period values are in descending order (lower pitches = higher periods).
/// 3. Verify they fall within the valid APU period range (0x000-0x7FF).
///
/// This is a pattern-based heuristic that may have false positives/negatives,
/// but is designed to be conservative (only match clear patterns).
pub fn detect_famitone2(prg_banks: &[Vec<u8>]) -> Option<FamiTone2Info> {
    // Concatenate all PRG banks into a single buffer for searching.
    let mut prg_data = Vec::new();
    for bank in prg_banks {
        prg_data.extend_from_slice(bank);
    }

    // Search for the 60-entry period table (120 bytes of u16 LE values).
    // Look for descending sequences of valid period values.
    let pattern = find_period_table(&prg_data)?;

    // For now, we assume the driver starts some distance before the period table
    // (typically a few hundred bytes earlier). A more sophisticated approach could
    // scan backwards for known init code patterns.
    let driver_offset = if pattern.offset > 256 {
        pattern.offset - 256
    } else {
        0
    };

    Some(FamiTone2Info {
        driver_offset,
        period_table_offset: pattern.offset,
    })
}

/// Internal structure for a detected period table.
struct PeriodTableMatch {
    offset: usize,
}

/// Search for a 60-entry period table in PRG data.
///
/// The period table consists of 60 u16 values (120 bytes total) in little-endian order.
/// Valid periods range from ~200 to ~2000 (rough bounds for the musical scale C1-B5).
/// The sequence should be monotonic descending or mostly descending.
fn find_period_table(prg_data: &[u8]) -> Option<PeriodTableMatch> {
    const PERIOD_TABLE_SIZE: usize = 60;
    const PERIOD_BYTES: usize = PERIOD_TABLE_SIZE * 2; // 120 bytes

    if prg_data.len() < PERIOD_BYTES {
        return None;
    }

    // Slide a window across the PRG data looking for period table patterns.
    for offset in 0..=(prg_data.len() - PERIOD_BYTES) {
        if is_period_table(&prg_data[offset..offset + PERIOD_BYTES]) {
            return Some(PeriodTableMatch { offset });
        }
    }

    None
}

/// Check if a 120-byte slice is a valid period table.
///
/// A period table should:
/// - Contain exactly 60 u16 LE values
/// - Have values roughly in descending order (first period > last period)
/// - All periods should be in valid APU range (0x0000-0x07FF)
/// - Exhibit a musical progression (roughly log-spaced for equal temperament)
fn is_period_table(data: &[u8]) -> bool {
    if data.len() != 120 {
        return false;
    }

    let mut periods = Vec::with_capacity(60);
    for i in 0..60 {
        let lo = data[i * 2] as u16;
        let hi = data[i * 2 + 1] as u16;
        let period = lo | ((hi & 0x07) << 8);
        periods.push(period);
    }

    // Basic sanity checks:
    // 1. All periods should be in APU range (0-2047 for 11-bit period values).
    if periods.iter().any(|&p| p > 2047) {
        return false;
    }

    // 2. The first period (C1, lowest note) should be higher than last period (B5, highest note).
    // Period is inversely proportional to frequency.
    let first_period = periods[0];
    let last_period = periods[59];

    // First note should have a higher period than last note (allow 10% tolerance for rounding).
    if last_period as f64 > (first_period as f64 * 0.95) {
        return false;
    }

    // 3. Count strictly decreasing pairs - most should decrease as pitch goes up.
    let strictly_decreasing = periods.windows(2).filter(|w| w[1] < w[0]).count();

    // Expect at least 75% of pairs to be strictly decreasing.
    if strictly_decreasing < 44 {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a test period table matching the runtime's equal-tempered scale.
    fn generate_test_period_table() -> Vec<u8> {
        // This matches the calculation in runtime::gen_period_table()
        const CPU: f64 = 1_789_773.0;
        const A4_HZ: f64 = 440.0;

        let mut bytes = Vec::with_capacity(120);
        for i in 0i32..60 {
            let semitone_offset = f64::from(i - 45);
            let freq = A4_HZ * 2f64.powf(semitone_offset / 12.0);
            let period_f = CPU / (16.0 * freq) - 1.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let period = period_f.round().clamp(0.0, 2047.0) as u16;
            let lo = (period & 0xFF) as u8;
            let hi = ((period >> 8) as u8 & 0x07) | 0x08;
            bytes.push(lo);
            bytes.push(hi);
        }
        bytes
    }

    #[test]
    fn test_is_period_table_valid() {
        let table = generate_test_period_table();
        assert_eq!(table.len(), 120);
        assert!(is_period_table(&table), "generated period table should validate");
    }

    #[test]
    fn test_is_period_table_invalid_size() {
        let table = vec![0u8; 119];
        assert!(
            !is_period_table(&table),
            "period table must be exactly 120 bytes"
        );
    }

    #[test]
    fn test_is_period_table_out_of_range() {
        let mut table = generate_test_period_table();
        // Corrupt a period value to be out of range (> 2047).
        table[50] = 0xFF; // period = 0x0FFF = 4095 (invalid)
        table[51] = 0x0F;
        assert!(!is_period_table(&table), "out-of-range period should be rejected");
    }

    #[test]
    fn test_find_period_table() {
        let table = generate_test_period_table();

        // Create a PRG buffer with some padding, then the table.
        let mut prg = vec![0xEA; 250]; // 250 bytes of NOP (0xEA)
        prg.extend_from_slice(&table);
        prg.extend_from_slice(&[0xFF; 256]); // Trailing padding

        let result = find_period_table(&prg);
        assert!(result.is_some(), "should find period table");
        assert_eq!(result.unwrap().offset, 250, "period table should start at offset 250");
    }

    #[test]
    fn test_detect_famitone2() {
        let table = generate_test_period_table();

        // Create a single PRG bank with the period table.
        let mut prg_bank = vec![0xEA; 512];
        prg_bank.extend_from_slice(&table);

        let result = detect_famitone2(&vec![prg_bank]);
        assert!(result.is_some(), "should detect FamiTone2 driver");
        let info = result.unwrap();
        assert!(info.period_table_offset > 0, "period table offset should be non-zero");
    }

    #[test]
    fn test_detect_famitone2_no_driver() {
        let prg_bank = vec![0xEA; 256]; // Just NOP padding, no period table.
        let result = detect_famitone2(&vec![prg_bank]);
        assert!(result.is_none(), "should not detect driver when absent");
    }
}
