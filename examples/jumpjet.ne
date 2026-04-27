// JumpJet — a NES port of the 1990 DOS shooter by Monte Variakojis
// (Montsoft).
//
// A Defender/Scramble-style side-scrolling shooter: you pilot a
// Harrier-like VTOL jet, shooting enemy planes with missiles
// (button A) and bombing tanks on the ground (button B). The
// D-pad changes altitude (up / down) and direction of flight
// (left / right).
//
// The source is split across examples/jumpjet/*.ne files; this
// top-level file declares the hardware config, palette, the one
// big Tileset CHR block, the background nametable, and the
// audio + state includes.
//
// Controls:
//     D-pad ↑ / ↓     — climb / dive
//     D-pad ← / →     — face left / face right
//     A               — fire missile (in facing direction)
//     B               — drop bomb (gravity-falls onto ground)
//     Start           — Title: confirm; GameOver: retry
//
// The headless emulator harness presses no buttons, so an
// autopilot keeps the game progressing past Title and produces
// recognisable mid-mission action at frame 180.
//
// Build:  cargo run --release -- build examples/jumpjet.ne
// Output: examples/jumpjet.nes

game "JumpJet" {
    mapper: NROM
    mirroring: horizontal
}

// ── Palette ──────────────────────────────────────────────────
//
// Grouped form so the universal sky-blue fills every sub-palette
// index 0 automatically (avoids the $3F10 mirror trap).
//
//     bg0 (sky)        : white clouds, soft gray accents
//     bg1 (ground)     : forest green grass + brown dirt for the
//                        horizon stripe and ground band
//     bg2 (HUD chrome) : white digits + red highlights, matching
//                        sp0 so sprite digits and bg digits look
//                        identical
//     bg3 (mountains)  : reserved for a future hill silhouette
//
// sp0 carries every sprite. Four colours that have to do triple
// duty:
//     1 = white   (jet hi-lights, missiles, alphabet, digits)
//     2 = red     (enemy planes, hearts, explosion flame)
//     3 = lt_gray (jet body, tank body, bomb, cloud body)
palette Main {
    universal: sky_blue

    bg0: [white,    lt_gray,  off_white]   // sky / clouds
    bg1: [forest,   dk_olive, brown]       // ground band
    bg2: [white,    red,      lt_gray]     // HUD chrome (matches sp0)
    bg3: [dk_gray,  gray,     lt_gray]     // reserved

    sp0: [white,    red,      lt_gray]
    sp1: [white,    red,      dk_red]      // reserved
    sp2: [white,    olive,    forest]      // reserved
    sp3: [white,    gray,     dk_gray]     // reserved
}

// Pull in everything else. Order matters only for symbol visibility:
// constants first (so subsequent files can reference TILE_* / MAX_*),
// then assets (Tileset), audio, state, render helpers, then the
// state handlers in transition order.
include "jumpjet/constants.ne"
include "jumpjet/assets.ne"
include "jumpjet/audio.ne"
include "jumpjet/state.ne"
include "jumpjet/render.ne"
include "jumpjet/title_state.ne"
include "jumpjet/play_state.ne"
include "jumpjet/gameover_state.ne"

// ── Background ──────────────────────────────────────────────
//
// One static 32×30 nametable: a flat sky band on top, a single
// horizon stripe at row 21, and a ground band filling rows 22-29.
// Per-region attribute palettes: HUD chrome on row 0-1, sky for
// the upper rows, ground for the bottom strip.
//
// `palette_map:` rows are 16 cells wide (each cell covers a 16×16
// metatile, so 15 rows drive the full 240-line screen).
background Stage {
    legend {
        ".": 1   // TILE_SKY
        "#": 2   // TILE_GROUND
        "=": 3   // TILE_HORIZON
    }

    map: [
        "................................",   //  0  — HUD area (sprites paint on top)
        "................................",   //  1
        "................................",   //  2
        "................................",   //  3
        "................................",   //  4
        "................................",   //  5
        "................................",   //  6
        "................................",   //  7
        "................................",   //  8
        "................................",   //  9
        "................................",   // 10
        "................................",   // 11
        "................................",   // 12
        "................................",   // 13
        "................................",   // 14
        "................................",   // 15
        "................................",   // 16
        "................................",   // 17
        "................................",   // 18
        "................................",   // 19
        "................................",   // 20
        "================================",   // 21 — horizon stripe
        "################################",   // 22
        "################################",   // 23
        "################################",   // 24
        "################################",   // 25
        "################################",   // 26
        "################################",   // 27
        "################################",   // 28
        "################################"    // 29
    ]

    palette_map: [
        "2222222222222222",   // metatile row 0 (NT rows 0-1)  → bg2 HUD chrome
        "0000000000000000",   // metatile rows 1-9             → bg0 sky
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "1111111111111111",   // metatile rows 10-14            → bg1 ground
        "1111111111111111",
        "1111111111111111",
        "1111111111111111",
        "1111111111111111"
    ]
}

start Title
