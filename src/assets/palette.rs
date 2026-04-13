/// NES color palette -- 64 colors as (R, G, B) tuples.
pub const NES_COLORS: [(u8, u8, u8); 64] = [
    // Row 0
    (84, 84, 84), // 0x00
    (0, 30, 116), // 0x01
    (8, 16, 144), // 0x02
    (48, 0, 136), // 0x03
    (68, 0, 100), // 0x04
    (92, 0, 48),  // 0x05
    (84, 4, 0),   // 0x06
    (60, 24, 0),  // 0x07
    (32, 42, 0),  // 0x08
    (8, 58, 0),   // 0x09
    (0, 64, 0),   // 0x0A
    (0, 60, 0),   // 0x0B
    (0, 50, 60),  // 0x0C
    (0, 0, 0),    // 0x0D
    (0, 0, 0),    // 0x0E
    (0, 0, 0),    // 0x0F
    // Row 1
    (152, 150, 152), // 0x10
    (8, 76, 196),    // 0x11
    (48, 50, 236),   // 0x12
    (92, 30, 228),   // 0x13
    (136, 20, 176),  // 0x14
    (160, 20, 100),  // 0x15
    (152, 34, 32),   // 0x16
    (120, 60, 0),    // 0x17
    (84, 90, 0),     // 0x18
    (40, 114, 0),    // 0x19
    (8, 124, 0),     // 0x1A
    (0, 118, 40),    // 0x1B
    (0, 102, 120),   // 0x1C
    (0, 0, 0),       // 0x1D
    (0, 0, 0),       // 0x1E
    (0, 0, 0),       // 0x1F
    // Row 2
    (236, 238, 236), // 0x20
    (76, 154, 236),  // 0x21
    (120, 124, 236), // 0x22
    (176, 98, 236),  // 0x23
    (228, 84, 236),  // 0x24
    (236, 88, 180),  // 0x25
    (236, 106, 100), // 0x26
    (212, 136, 32),  // 0x27
    (160, 170, 0),   // 0x28
    (116, 196, 0),   // 0x29
    (76, 208, 32),   // 0x2A
    (56, 204, 108),  // 0x2B
    (56, 180, 204),  // 0x2C
    (60, 60, 60),    // 0x2D
    (0, 0, 0),       // 0x2E
    (0, 0, 0),       // 0x2F
    // Row 3
    (236, 238, 236), // 0x30
    (168, 204, 236), // 0x31
    (188, 188, 236), // 0x32
    (212, 178, 236), // 0x33
    (236, 174, 236), // 0x34
    (236, 174, 212), // 0x35
    (236, 180, 176), // 0x36
    (228, 196, 144), // 0x37
    (204, 210, 120), // 0x38
    (180, 222, 120), // 0x39
    (168, 226, 144), // 0x3A
    (152, 226, 180), // 0x3B
    (160, 214, 228), // 0x3C
    (160, 162, 160), // 0x3D
    (0, 0, 0),       // 0x3E
    (0, 0, 0),       // 0x3F
];

/// Find the nearest NES color index for an RGB value.
pub fn nearest_nes_color(r: u8, g: u8, b: u8) -> u8 {
    let mut best_idx = 0u8;
    let mut best_dist = u32::MAX;
    for (i, &(nr, ng, nb)) in NES_COLORS.iter().enumerate() {
        let dr = i32::from(r) - i32::from(nr);
        let dg = i32::from(g) - i32::from(ng);
        let db = i32::from(b) - i32::from(nb);
        let dist = (dr * dr + dg * dg + db * db).unsigned_abs();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i as u8;
        }
    }
    best_idx
}

/// Resolve a human-readable color name to its NES master palette index
/// (`$00-$3F`). Returns `None` for unknown names.
///
/// The name list is a curated subset of the 64-entry master palette,
/// chosen for how distinct each colour is in practice (rows 3 and
/// above often produce near-duplicates so we skip most of them). Names
/// are case-insensitive; underscores and hyphens are interchangeable.
///
/// Every NES programmer eventually memorizes that `$0F` is "the one
/// true black" — the one hardware palette index guaranteed to render
/// as `(0,0,0)` on every TV — so `black` maps to `$0F` rather than
/// `$1D`/`$2E`/`$3E`/`$3F` (which are also black but are commonly used
/// as "emphasis blanking" slots in advanced code).
#[must_use]
pub fn color_name_to_index(name: &str) -> Option<u8> {
    // Normalize: lowercase + collapse `-` to `_`.
    let normalized: String = name
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    Some(match normalized.as_str() {
        // ── Grayscale ──
        // $0F is the canonical "true black" slot — preferred over
        // $1D/$2E/$3E/$3F which are mirrors used for emphasis blanking.
        "black" => 0x0F,
        "dk_gray" | "dark_gray" | "darkgray" => 0x00,
        "gray" | "grey" | "mid_gray" => 0x10,
        "lt_gray" | "light_gray" | "lightgray" => 0x3D,
        "white" => 0x30,
        "off_white" | "pale_white" => 0x20,

        // ── Blues ──
        "dk_blue" | "dark_blue" | "navy" => 0x01,
        "blue" => 0x11,
        "lt_blue" | "light_blue" | "sky_blue" | "sky" => 0x21,
        "pale_blue" => 0x31,
        "indigo" => 0x02,
        "royal_blue" | "royal" => 0x12,
        "periwinkle" => 0x22,
        "ice_blue" | "ice" => 0x32,

        // ── Purples / magentas ──
        "dk_purple" | "dark_purple" => 0x03,
        "purple" | "violet" => 0x13,
        "lt_purple" | "light_purple" | "lavender" => 0x23,
        "pale_purple" => 0x33,
        "dk_magenta" | "dark_magenta" => 0x04,
        "magenta" => 0x14,
        "lt_magenta" | "light_magenta" | "pink" => 0x24,
        "pale_pink" => 0x34,

        // ── Pinks / roses ──
        "dk_rose" | "maroon" => 0x05,
        "rose" => 0x15,
        "hot_pink" => 0x25,
        "pale_rose" => 0x35,

        // ── Reds ──
        "dk_red" | "dark_red" => 0x06,
        "red" => 0x16,
        "lt_red" | "light_red" => 0x26,
        "peach" => 0x36,

        // ── Oranges / browns ──
        "brown" => 0x07,
        "dk_orange" | "dark_orange" => 0x17,
        "orange" => 0x27,
        "tan" => 0x37,

        // ── Yellows ──
        "dk_olive" | "dark_olive" => 0x08,
        "olive" => 0x18,
        "yellow" => 0x28,
        "lt_yellow" | "light_yellow" | "cream" => 0x38,

        // ── Greens ──
        "dk_green" | "dark_green" => 0x09,
        "green" => 0x19,
        "lt_green" | "light_green" | "lime" => 0x29,
        "pale_green" => 0x39,
        "forest" | "forest_green" => 0x0A,
        "bright_green" => 0x1A,
        "neon_green" => 0x2A,
        "mint" => 0x3A,

        // ── Teals / cyans ──
        "dk_teal" | "dark_teal" => 0x0B,
        "teal" => 0x1B,
        "lt_teal" | "light_teal" | "aqua" => 0x2B,
        "pale_teal" | "pale_aqua" => 0x3B,
        "dk_cyan" | "dark_cyan" => 0x0C,
        "cyan" => 0x1C,
        "lt_cyan" | "light_cyan" => 0x2C,
        "pale_cyan" => 0x3C,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_is_true_black() {
        // $0F is the one-true-black slot. Every NES programmer relies
        // on this being the default bg colour, so it must stay pinned.
        assert_eq!(color_name_to_index("black"), Some(0x0F));
    }

    #[test]
    fn names_are_case_insensitive() {
        assert_eq!(color_name_to_index("RED"), Some(0x16));
        assert_eq!(color_name_to_index("Red"), Some(0x16));
        assert_eq!(color_name_to_index("red"), Some(0x16));
    }

    #[test]
    fn aliases_resolve_identically() {
        // Grey / gray and light_gray / lt_gray should all map to the
        // same hardware slot so users don't have to remember which
        // spelling we picked.
        assert_eq!(color_name_to_index("gray"), color_name_to_index("grey"));
        assert_eq!(
            color_name_to_index("lt_gray"),
            color_name_to_index("light_gray")
        );
        assert_eq!(
            color_name_to_index("dk_blue"),
            color_name_to_index("dark_blue")
        );
    }

    #[test]
    fn hyphen_separator_also_works() {
        // `dark-red` and `dark_red` should mean the same thing so a
        // user copying CSS-style names doesn't get surprising errors.
        assert_eq!(
            color_name_to_index("dark-red"),
            color_name_to_index("dark_red")
        );
    }

    #[test]
    fn unknown_name_returns_none() {
        assert_eq!(color_name_to_index("mauve"), None);
        assert_eq!(color_name_to_index(""), None);
    }

    #[test]
    fn every_returned_index_is_in_master_palette_range() {
        for name in [
            "black", "white", "red", "green", "blue", "yellow", "orange", "purple", "cyan", "teal",
            "brown", "olive", "tan", "mint", "peach", "indigo",
        ] {
            let idx = color_name_to_index(name).expect(name);
            assert!(idx <= 0x3F, "{name} -> {idx:#04x} out of range");
        }
    }
}
