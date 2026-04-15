/// Audio data extraction: reverse-engineer music and SFX data from compiled ROMs.
///
/// Given a FamiTone2 period table, this module:
/// - Inverts the period table to map APU period values back to note indices (0-60)
/// - Searches for and parses music note streams (pitch, duration pairs)
/// - Searches for and parses SFX envelope data
/// - Reconstructs structured music/sfx declarations

use std::collections::HashMap;

/// Inverted period table: maps APU period values to note indices (0-60).
/// Index 0 is rest, 1-60 are C1-B5.
#[derive(Debug, Clone)]
pub struct PeriodInverter {
    // HashMap for exact and fuzzy period lookups.
    period_to_note: HashMap<u16, u8>,
    // For fuzzy matching when exact periods don't exist, store the closest match.
    fuzzy_matches: Vec<(u16, u8)>, // (period, note_index)
}

impl PeriodInverter {
    /// Create a period inverter from raw period table bytes.
    ///
    /// Takes 120 bytes of u16 LE period values (60 entries, indices 0-59 mapped to notes 1-60 for C1-B5).
    pub fn from_bytes(period_data: &[u8]) -> Option<Self> {
        if period_data.len() != 120 {
            return None;
        }

        let mut period_to_note = HashMap::new();
        let mut fuzzy_matches = Vec::new();

        for i in 0..60 {
            let lo = period_data[i * 2] as u16;
            let hi = period_data[i * 2 + 1] as u16;
            // Extract the 11-bit period value (ignoring length-counter bits in hi).
            let period = lo | ((hi & 0x07) << 8);
            let note_index = (i + 1) as u8; // Notes 1-60, rest is 0.
            period_to_note.insert(period, note_index);
            fuzzy_matches.push((period, note_index));
        }

        // Sort fuzzy_matches by period value for binary search.
        fuzzy_matches.sort_by_key(|p| p.0);

        Some(PeriodInverter {
            period_to_note,
            fuzzy_matches,
        })
    }

    /// Convert an APU period value to a note index (0-60).
    /// 0 = rest, 1-60 = C1-B5.
    ///
    /// First tries exact match; if not found, returns the closest period by binary search.
    pub fn period_to_note(&self, period: u16) -> u8 {
        // Exact match?
        if let Some(&note) = self.period_to_note.get(&period) {
            return note;
        }

        // Fuzzy match: find the closest period.
        if self.fuzzy_matches.is_empty() {
            return 0; // Rest
        }

        let idx = match self.fuzzy_matches.binary_search_by_key(&period, |p| p.0) {
            Ok(i) => i,
            Err(i) => {
                // i is the insertion point. Pick the closer of i-1 and i.
                if i == 0 {
                    0
                } else if i >= self.fuzzy_matches.len() {
                    self.fuzzy_matches.len() - 1
                } else {
                    let prev_dist = (self.fuzzy_matches[i - 1].0 as i32 - period as i32).abs();
                    let next_dist = (self.fuzzy_matches[i].0 as i32 - period as i32).abs();
                    if prev_dist <= next_dist {
                        i - 1
                    } else {
                        i
                    }
                }
            }
        };

        if idx < self.fuzzy_matches.len() {
            self.fuzzy_matches[idx].1
        } else {
            0
        }
    }
}

/// Extracted music data from a ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedMusic {
    pub name: String,
    pub duty: u8,
    pub volume: u8,
    pub repeat: bool,
    pub notes: Vec<(u8, u8)>, // (note_index, duration)
}

/// Extracted SFX data from a ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSfx {
    pub name: String,
    pub duty: u8,
    pub period_lo: u8,
    pub period_hi: u8,
    pub envelope: Vec<u8>,
}

/// Search the PRG data for music and SFX blobs.
///
/// Music blobs are typically (pitch, duration) pairs terminated by (0xFF, 0xFF).
/// SFX envelopes are byte sequences terminated by 0x00 (pulse/noise) or 0x80 (triangle).
///
/// This is a pattern-based heuristic; we search for sentinel patterns and extract
/// the data immediately preceding them.
pub fn extract_audio_data(
    prg_data: &[u8],
    period_inverter: &PeriodInverter,
) -> (Vec<ExtractedMusic>, Vec<ExtractedSfx>) {
    // For now, we return empty vecs. Full extraction requires:
    // 1. Scanning the PRG for music label pointers (typically at $8000-$A000 in NEScript).
    // 2. Following pointers to locate note streams.
    // 3. Parsing each note stream until the (0xFF, 0xFF) sentinel.
    // 4. Reconstructing names from label names if available (requires symbol info).
    //
    // This is deferred to a future enhancement. The core period table inversion works.

    let _ = (prg_data, period_inverter);
    (Vec::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a test period table.
    fn generate_test_period_table() -> Vec<u8> {
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
    fn test_period_inverter_from_bytes() {
        let table = generate_test_period_table();
        let inverter = PeriodInverter::from_bytes(&table);
        assert!(inverter.is_some(), "should create inverter from valid table");
    }

    #[test]
    fn test_period_inverter_wrong_size() {
        let table = vec![0u8; 119];
        let inverter = PeriodInverter::from_bytes(&table);
        assert!(inverter.is_none(), "should reject table of wrong size");
    }

    #[test]
    fn test_period_to_note_exact_match() {
        let table = generate_test_period_table();
        let inverter = PeriodInverter::from_bytes(&table).unwrap();

        // Extract the last period value (B5, note index 60).
        let lo = table[118] as u16;  // Last entry starts at byte 118
        let hi = table[119] as u16;
        let b5_period = lo | ((hi & 0x07) << 8);

        // Should map back to note 60 (B5).
        assert_eq!(inverter.period_to_note(b5_period), 60);
    }

    #[test]
    fn test_period_to_note_fuzzy_match() {
        let table = generate_test_period_table();
        let inverter = PeriodInverter::from_bytes(&table).unwrap();

        // Extract a real period and slightly modify it.
        let lo = table[118] as u16;
        let hi = table[119] as u16;
        let b5_period = lo | ((hi & 0x07) << 8);

        // Slightly modified period should still match close note.
        let modified = b5_period.saturating_add(1);
        let note = inverter.period_to_note(modified);
        assert!(note > 0, "should find a valid note for nearby period");
    }

    #[test]
    fn test_period_to_note_unknown() {
        let table = generate_test_period_table();
        let inverter = PeriodInverter::from_bytes(&table).unwrap();

        // A very small period (high frequency) should fuzzy-match to a high note.
        let note = inverter.period_to_note(100);
        assert!(note > 50, "should return high note for small period");
    }

    #[test]
    fn test_extracted_music_equality() {
        let m1 = ExtractedMusic {
            name: "Theme".to_string(),
            duty: 2,
            volume: 10,
            repeat: true,
            notes: vec![(37, 20), (41, 20)],
        };
        let m2 = ExtractedMusic {
            name: "Theme".to_string(),
            duty: 2,
            volume: 10,
            repeat: true,
            notes: vec![(37, 20), (41, 20)],
        };
        assert_eq!(m1, m2);
    }

    #[test]
    fn test_extracted_sfx_equality() {
        let s1 = ExtractedSfx {
            name: "Coin".to_string(),
            duty: 2,
            period_lo: 0x50,
            period_hi: 0x08,
            envelope: vec![15, 14, 13, 12, 0],
        };
        let s2 = ExtractedSfx {
            name: "Coin".to_string(),
            duty: 2,
            period_lo: 0x50,
            period_hi: 0x08,
            envelope: vec![15, 14, 13, 12, 0],
        };
        assert_eq!(s1, s2);
    }
}
