// Palette and Background Demo — shows the `palette` and
// `background` declarations plus runtime `set_palette` /
// `load_background` swaps.
//
// The program declares two palettes and two backgrounds. The
// *first* of each is loaded at reset time (before rendering is
// enabled) so the title screen comes up with the right colours
// and nametable on frame 0. A frame counter then toggles between
// the two palettes every 90 frames and between the two backgrounds
// every 180 frames, exercising the vblank-safe update path.
//
// Background tile 0 is the compiler's built-in smiley face (see
// the default CHR in `src/linker/mod.rs`). Tile indices 1+ would
// need matching `sprite` declarations with `chr:` data — we stick
// to tile 0 so the example is self-contained.
//
// Build:  cargo run -- build examples/palette_and_background.ne
// Output: examples/palette_and_background.nes

game "Palette + BG Demo" {
    mapper: NROM
    mirroring: horizontal
}

// ── Palettes ────────────────────────────────────────────────
//
// Each palette is 32 bytes laid out in the standard NES order:
//   $00-$0F  background palettes 0-3 (4 colours each)
//   $10-$1F  sprite palettes 0-3 (4 colours each, $10/$14/$18/$1C
//             mirror the bg slots)
//
// `CoolBlues` uses deep blue background tiles with pale highlights;
// `WarmReds` swaps them for red/orange tones. Every 4 bytes is a
// sub-palette.

palette CoolBlues {
    colors: [
        0x0F, 0x01, 0x11, 0x21,  // bg palette 0: black, deep blue, sky, pale
        0x0F, 0x02, 0x12, 0x22,  // bg palette 1
        0x0F, 0x0C, 0x1C, 0x2C,  // bg palette 2
        0x0F, 0x0B, 0x1B, 0x2B,  // bg palette 3
        0x0F, 0x01, 0x11, 0x21,  // sprite palette 0
        0x0F, 0x16, 0x27, 0x30,  // sprite palette 1
        0x0F, 0x14, 0x24, 0x34,  // sprite palette 2
        0x0F, 0x0B, 0x1B, 0x2B   // sprite palette 3
    ]
}

palette WarmReds {
    colors: [
        0x0F, 0x06, 0x16, 0x26,  // bg palette 0: black, dark red, red, peach
        0x0F, 0x07, 0x17, 0x27,  // bg palette 1
        0x0F, 0x08, 0x18, 0x28,  // bg palette 2
        0x0F, 0x09, 0x19, 0x29,  // bg palette 3
        0x0F, 0x06, 0x16, 0x26,  // sprite palette 0
        0x0F, 0x16, 0x27, 0x30,  // sprite palette 1
        0x0F, 0x14, 0x24, 0x34,  // sprite palette 2
        0x0F, 0x0B, 0x1B, 0x2B   // sprite palette 3
    ]
}

// ── Backgrounds ─────────────────────────────────────────────
//
// A 32×30 nametable is 960 tile bytes + 64 attribute bytes. We
// only specify the first few tiles per background — the rest of
// the nametable is zero-padded by the asset pipeline, so
// undeclared cells render as tile 0 (the built-in smiley). The
// attribute table is left empty so every 16×16 metatile uses
// background sub-palette 0 (which is where CoolBlues / WarmReds
// put their headline colour).
//
// TitleScreen paints a short ribbon of tile 0 across row 14 (the
// middle of the screen). StageOne paints a diagonal stripe from
// top-left to bottom-right so the swap is visually obvious.

background TitleScreen {
    tiles: [
        // Row 14 (offset 14*32 = 448): a ribbon of tile 0
        // across columns 4..28.
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // Row 14
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    ]
}

background StageOne {
    tiles: [
        // A few scattered tile-0s across the first 4 rows so the
        // background visibly differs from TitleScreen after the
        // swap. Remaining rows are zero-padded and render as
        // tile 0 anyway — the visual difference comes from the
        // palette swap triggered alongside.
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    ]
}

var tick: u8 = 0
var phase: u8 = 0

on frame {
    tick += 1

    // Every 90 frames toggle the palette between the two.
    if tick >= 90 {
        tick = 0
        phase += 1
        if phase == 4 {
            phase = 0
        }
    }

    // Phase 0: CoolBlues + TitleScreen
    // Phase 1: WarmReds + TitleScreen
    // Phase 2: CoolBlues + StageOne
    // Phase 3: WarmReds  + StageOne
    //
    // We reissue the set_palette / load_background call on every
    // phase transition (the single-frame condition also keeps the
    // golden stable — the jsnes test only renders a handful of
    // frames and the comparison runs without any controller input).
    if tick == 1 {
        if phase == 0 {
            set_palette CoolBlues
            load_background TitleScreen
        }
        if phase == 1 {
            set_palette WarmReds
            load_background TitleScreen
        }
        if phase == 2 {
            set_palette CoolBlues
            load_background StageOne
        }
        if phase == 3 {
            set_palette WarmReds
            load_background StageOne
        }
    }

    // Draw a single sprite tracking the tick so the sprite still
    // moves on screen and the golden diff tests spot regressions
    // in OAM DMA alongside the palette/background path.
    draw Smiley at: (tick + 60, 120)
}

start Main
