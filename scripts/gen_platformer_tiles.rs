//! One-shot generator for the CHR tiles and nametable used by
//! `examples/platformer.ne`.
//!
//! Run with `cargo run --bin gen_platformer_tiles`. The output is
//! intended to be pasted into `platformer.ne` under the
//! `sprite Tileset { chr: [...] }` and
//! `background Level { tiles: [...] }` blocks. Keeping the source of
//! truth here (instead of hand-maintained hex in the `.ne` file)
//! makes the tile art editable as ASCII and ensures the CHR bytes
//! and the nametable stay in sync with named tile indices.
//!
//! Tiles are defined as 8×8 ASCII art where each character selects
//! one of 4 sub-palette slots:
//!     '.' = colour 0  (sky / transparent for sprites)
//!     'a' = colour 1
//!     'b' = colour 2
//!     'c' = colour 3

use std::fmt::Write as _;

/// A single 8×8 tile description: name + ASCII art.
struct Tile {
    name: &'static str,
    art: &'static str,
}

/// All tiles referenced by the platformer example. Order matters —
/// tile 0 is the default smiley reserved by the linker, so entry 0
/// here lands at CHR index 1, entry 1 at index 2, and so on.
///
/// Colour conventions (must line up with the palette + attribute
/// layout in `examples/platformer.ne`):
///   sprite sub-palette 0 (player / enemy / coin):
///     a = red/cap, b = peach/skin, c = white/highlight
///   bg sub-palette 0 (sky row): a = white, b = light gray, c = black
///   bg sub-palette 1 (block row): a = red, b = peach, c = dark red
///   bg sub-palette 2 (ground row): a = light green, b = light brown,
///                                   c = dark brown
const TILES: &[Tile] = &[
    Tile {
        name: "Player head L",
        art: "\
..aaaaaa
.aaaaaaa
.aaabbbb
.aabbbbc
aabbcccc
aabbccbc
aabbbbbb
aabbbbbb",
    },
    Tile {
        name: "Player head R",
        art: "\
aaaaaa..
aaaaaaa.
bbbbaaa.
cbbbbbaa
cccccbbb
bcbbccbb
bbbbbbbb
bbbbbbbb",
    },
    Tile {
        name: "Player body L",
        art: "\
.aaaaaaa
aaacaaaa
aaaccaaa
aaaaaaaa
.bbbbbbb
.bbbbbbb
.bb..bbb
aaa..bbb",
    },
    Tile {
        name: "Player body R",
        art: "\
aaaaaaa.
aaaacaaa
aaaccaaa
aaaaaaaa
bbbbbbb.
bbbbbbb.
bbb..bb.
bbb..aaa",
    },
    Tile {
        name: "Enemy",
        art: "\
..aaaa..
.aaaaaa.
.aacbaa.
aacccaaa
abbccbba
.aaaaaa.
a.aaaa.a
a..aa..a",
    },
    Tile {
        name: "Coin",
        art: "\
...bb...
..bbbb..
.bbccbb.
.bcbbcb.
.bcbbcb.
.bbccbb.
..bbbb..
...bb...",
    },
    // Grass top (sub-palette 2: a=green, c=dark brown).
    Tile {
        name: "Grass top",
        art: "\
aaaaaaaa
a.aaa.aa
aaaaaaaa
accacaca
ccccaccc
cccccccc
cbccccbc
cccccccc",
    },
    // Dirt (sub-palette 2: c=dark brown bulk, b=light brown speckles).
    Tile {
        name: "Dirt",
        art: "\
cccccccc
cbccccbc
cccccccc
ccccbccc
cccccccc
cbccccbc
ccccbccc
cccccccc",
    },
    // Brick (sub-palette 1: a=red, c=dark red mortar).
    Tile {
        name: "Brick",
        art: "\
cccccccc
caaaaaca
caaaaaca
caaaaaca
cccccccc
aaacaaaa
aaacaaaa
aaacaaaa",
    },
    // Cloud left (sub-palette 0: a=white, b=light gray shade).
    Tile {
        name: "Cloud L",
        art: "\
........
...aaa..
..aaaaa.
.aaaaaaa
.aaaaaaa
.bbaaaaa
..bbbbba
.....bbb",
    },
    // Cloud right (sub-palette 0).
    Tile {
        name: "Cloud R",
        art: "\
........
.aaa....
aaaaa...
aaaaaaa.
aaaaaaa.
aaaaabb.
abbbbb..
bbb.....",
    },
    // Hill (sub-palette 2: a=green, b=light brown shade).
    Tile {
        name: "Hill",
        art: "\
........
....aa..
...aaaa.
..aaaaaa
..abaaaa
.abbabaa
.aaababa
aabbabba",
    },
    // Bush (sub-palette 2: a=green, c=dark brown outline).
    Tile {
        name: "Bush",
        art: "\
........
...aa...
..aaaa..
.aaaaac.
aaaaaaca
aaaaacca
aaaaacca
acacacac",
    },
    // Q Block (sub-palette 1: a=red frame, b=peach face, c=dark red).
    Tile {
        name: "Q Block",
        art: "\
cccccccc
caaaaaac
cabbbbac
cabccbac
cabcbbac
cabbbbac
caaaaaac
cccccccc",
    },
    // Sky (all transparent — renders as the universal bg colour).
    Tile {
        name: "Sky (blank)",
        art: "\
........
........
........
........
........
........
........
........",
    },
];

// ── Named CHR tile indices used by the nametable layout below ──
// (Player/enemy/coin tile indices are referenced by name only in
// the .ne file's `draw` statements, not by the nametable here, so
// the nametable-only constants live here.)
const GRASS: u8 = 7;
const DIRT: u8 = 8;
const BRICK: u8 = 9;
const CLOUD_L: u8 = 10;
const CLOUD_R: u8 = 11;
const HILL: u8 = 12;
const BUSH: u8 = 13;
const QBLOCK: u8 = 14;
const SKY: u8 = 15;

/// Encode one 8×8 tile into its 16-byte NES CHR representation
/// (two 8-byte bitplanes: low bit then high bit).
fn tile_to_chr(art: &str) -> [u8; 16] {
    let rows: Vec<&str> = art.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(rows.len(), 8, "expected 8 rows, got {}", rows.len());
    let mut chr = [0u8; 16];
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(row.len(), 8, "expected 8 cols in row {y}: {row:?}");
        let (mut plane0, mut plane1) = (0u8, 0u8);
        for (x, ch) in row.chars().enumerate() {
            let idx: u8 = match ch {
                '.' => 0,
                'a' => 1,
                'b' => 2,
                'c' => 3,
                other => panic!("invalid tile char {other:?}"),
            };
            if idx & 1 != 0 {
                plane0 |= 0x80 >> x;
            }
            if idx & 2 != 0 {
                plane1 |= 0x80 >> x;
            }
        }
        chr[y] = plane0;
        chr[y + 8] = plane1;
    }
    chr
}

/// Build a 32×30 nametable for a static Mario-ish level vista.
/// Horizontal scrolling pans this single nametable via `scroll()`,
/// so it only needs to look interesting at any X offset.
fn build_nametable() -> [u8; 960] {
    let mut nt = [SKY; 960];
    let set = |nt: &mut [u8; 960], y: usize, x: usize, t: u8| {
        nt[y * 32 + x] = t;
    };

    // Row 5 & 6: two clouds at different X positions for parallax feel.
    for &cx in &[4usize, 20] {
        set(&mut nt, 5, cx, CLOUD_L);
        set(&mut nt, 5, cx + 1, CLOUD_R);
    }
    set(&mut nt, 6, 12, CLOUD_L);
    set(&mut nt, 6, 13, CLOUD_R);

    // Row 15: a suspended brick platform with a Q-block in the middle.
    for x in 6..=9 {
        let t = if x == 8 { QBLOCK } else { BRICK };
        set(&mut nt, 15, x, t);
    }
    for x in 18..=20 {
        set(&mut nt, 15, x, BRICK);
    }

    // Row 20: hills sitting behind the grass line.
    for x in 2..=4 {
        set(&mut nt, 20, x, HILL);
    }
    for x in 22..=25 {
        set(&mut nt, 20, x, HILL);
    }

    // Row 21: bushes on top of the grass.
    for x in 10..=12 {
        set(&mut nt, 21, x, BUSH);
    }
    for x in 27..=28 {
        set(&mut nt, 21, x, BUSH);
    }

    // Row 22: grass top — a full horizontal line.
    for x in 0..32 {
        set(&mut nt, 22, x, GRASS);
    }

    // Row 23-29: dirt rows with a few buried bricks for texture.
    for y in 23..30 {
        for x in 0..32 {
            set(&mut nt, y, x, DIRT);
        }
    }
    for &(y, x) in &[(24usize, 5usize), (25, 11), (24, 18), (25, 24), (23, 30)] {
        set(&mut nt, y, x, BRICK);
    }

    nt
}

/// Build a 64-byte attribute table that pairs every 2×2 metatile
/// with a background sub-palette.
///
/// The attribute table covers a 16×15 grid of 16×16 metatiles. Each
/// byte encodes 4 quadrants (top-left, top-right, bottom-left,
/// bottom-right) of a 32×32-pixel "super-metatile" with 2 bits
/// apiece (palette index 0..3). For this demo we simply pick a
/// sub-palette per screen row region:
/// - top region (sky): sub-palette 0
/// - mid region (hills/blocks): sub-palette 1
/// - ground region (grass/dirt): sub-palette 2
fn build_attributes() -> [u8; 64] {
    let mut attr = [0u8; 64];
    // The attribute table is 8×8 bytes. Row `ay` covers nametable
    // rows 4*ay..4*ay+3. Palette layout:
    //   rows 0-11 (ay 0-2): sub-palette 0 (sky/clouds: whites)
    //   rows 12-15 (ay 3):  sub-palette 1 (brick/Q-block row)
    //   rows 16-19 (ay 4):  sub-palette 0 (more sky above the hills)
    //   rows 20-29 (ay 5-7): sub-palette 2 (grass/hills/bushes/dirt)
    for ay in 0..8 {
        let pal: u8 = match ay {
            0..=2 => 0,
            3 => 1,
            4 => 0,
            _ => 2, // 5, 6, 7
        };
        let quad = pal & 0b11;
        let byte = quad | (quad << 2) | (quad << 4) | (quad << 6);
        for ax in 0..8 {
            attr[ay * 8 + ax] = byte;
        }
    }
    attr
}

fn emit_byte_block(bs: &[u8], per_row: usize, indent: &str) -> String {
    let mut out = String::new();
    for chunk in bs.chunks(per_row) {
        out.push_str(indent);
        for (i, b) in chunk.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{b}");
        }
        out.push_str(",\n");
    }
    out
}

fn emit_hex_block(bs: &[u8], per_row: usize, indent: &str) -> String {
    let mut out = String::new();
    for chunk in bs.chunks(per_row) {
        out.push_str(indent);
        for (i, b) in chunk.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "0x{b:02X}");
        }
        out.push_str(",\n");
    }
    out
}

fn main() {
    // ── CHR ──
    let mut chr_all: Vec<u8> = Vec::new();
    println!("// ── CHR tiles (paste into sprite Tileset {{ chr: [...] }}) ──");
    for (i, tile) in TILES.iter().enumerate() {
        let tile_idx = i + 1;
        let chr = tile_to_chr(tile.art);
        chr_all.extend_from_slice(&chr);
        println!("    // tile {tile_idx}: {}", tile.name);
        print!("{}", emit_hex_block(&chr, 16, "    "));
    }
    println!(
        "// total tiles: {}, total CHR bytes: {}\n",
        TILES.len(),
        chr_all.len()
    );

    // ── Nametable ──
    let nt = build_nametable();
    assert_eq!(nt.len(), 960);
    println!("// ── Nametable (paste into background Level {{ tiles: [...] }}) ──");
    print!("{}", emit_byte_block(&nt, 16, "        "));

    // ── Attributes ──
    let attr = build_attributes();
    assert_eq!(attr.len(), 64);
    println!("\n// ── Attributes (paste into background Level {{ attributes: [...] }}) ──");
    print!("{}", emit_byte_block(&attr, 16, "        "));
}
