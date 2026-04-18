// HUD demo — the VRAM update buffer driving a classic status-bar
// layout above a scrolling playfield.
//
// The playfield shows a ball bouncing back and forth; every wall
// hit bumps a score counter that the HUD renders at the top of
// the screen. A "lives" indicator to the left ticks down
// periodically and resets, demonstrating both `nt_set` (for
// single-cell updates when state changes) and `nt_fill_h` (for
// repeatedly painting a multi-cell indicator bar). An `nt_attr`
// call at reset gives the HUD row a distinct colour palette so
// it reads as "UI chrome" instead of "gameplay background".
//
// The HUD never touches `$2006` / `$2007` directly — user code
// just appends records to the 256-byte ring at `$0400-$04FF`
// and the NMI handler drains them during vblank. `nt_attr` / `nt_set`
// / `nt_fill_h` each cost one buffer entry (4..19 bytes); the
// ~2273-cycle vblank budget drains ~200 single-cell writes, so a
// full HUD refresh fits comfortably.
//
// Only cells that *change* need a buffer entry: the demo tracks
// `last_score` and `last_lives` so the common "state didn't
// change this frame" path appends nothing. That's the whole
// point of the update buffer — per-frame cost scales with what
// actually changed, not with HUD complexity.

game "HUD Demo" {
    mapper: NROM
    mirroring: horizontal
}

palette GameColors {
    universal: black
    bg0: [dk_blue,  blue,     sky_blue]    // playfield background
    bg1: [dk_red,   red,      lt_red]      // HUD row
    bg2: [dk_green, green,    lt_green]
    bg3: [dk_gray,  lt_gray,  white]
    sp0: [dk_blue,  blue,     sky_blue]
    sp1: [red,      orange,   white]
    sp2: [dk_teal,  teal,     lt_teal]
    sp3: [dk_olive, olive,    yellow]
}

background Playfield {
    legend { ".": 0 }
    // Fill the nametable with tile 0 so sprite-0 hit plumbing
    // works and the HUD cells have opaque source pixels when we
    // overwrite them. The map's three rows zero-pad to the
    // full 30-row nametable automatically.
    map: [
        "................................",
        "................................",
        "................................"
    ]
}

// Ball position + velocity.
var bx: u8 = 64
var by: u8 = 100
var dx: u8 = 1         // 1 = moving right, 0 = moving left
var dy: u8 = 1         // 1 = moving down,  0 = moving up

// HUD state. `score` increments on each wall bounce; `lives`
// ticks down once a second and resets from 5 when it hits zero.
// The `last_*` shadows let us skip the HUD write when nothing
// has changed this frame.
var score:       u8 = 0
var lives:       u8 = 5
var last_score:  u8 = 255   // 255 forces an initial paint on frame 0
var last_lives:  u8 = 255
var life_tick:   u8 = 0
var attr_set:    u8 = 0     // 1 once the HUD attribute has been painted

on frame {
    // ── One-shot: paint the HUD attribute byte on the first
    //    frame only. The top metatile group picks up sub-palette
    //    1 (the red HUD palette) instead of sub-palette 0 (the
    //    blue playfield). `0b01010101` means all four 16×16
    //    quadrants of the metatile use sub-palette 1.
    if attr_set == 0 {
        nt_attr(0, 0, 0b01010101)
        attr_set = 1
    }

    // ── Playfield: bounce the ball, count bounces as score. ──
    if dx == 1 {
        bx += 1
        if bx >= 240 {
            dx = 0
            score += 1
        }
    } else {
        bx -= 1
        if bx == 16 {
            dx = 1
            score += 1
        }
    }
    if dy == 1 {
        by += 1
        if by >= 200 { dy = 0 }
    } else {
        by -= 1
        if by == 32 { dy = 1 }
    }
    draw Ball at: (bx, by)

    // ── HUD: tick the lives timer. One "life" ticks every 60
    //        frames (~1 sec); at zero, reset to 5. ──
    life_tick += 1
    if life_tick >= 60 {
        life_tick = 0
        if lives == 0 {
            lives = 5
        } else {
            lives -= 1
        }
    }

    // ── HUD: rewrite only the cells whose backing state changed.
    //        When `score` ticks, update the single-digit cell at
    //        (28, 1) via `nt_set`. When `lives` ticks, repaint
    //        the whole lives bar via `nt_fill_h` (5 cells
    //        starting at column 2 of row 1, filled with tile 0
    //        for "alive"). A pre-flight shadow-compare keeps
    //        the buffer empty on the ~58 of 60 frames when
    //        nothing changed.
    if score != last_score {
        last_score = score
        // Low nibble of score → tile index 0..15. A production
        // HUD would use CHR-authored digit glyphs; this demo
        // relies on the fact that whatever tiles happen to live
        // at CHR indices 0..15 will render as *visible* changes
        // frame-to-frame, making the update mechanism obvious.
        var digit: u8 = score & 0x0F
        nt_set(28, 1, digit)
    }
    if lives != last_lives {
        last_lives = lives
        // Fill `lives` cells at (2, 1) with the smiley tile, then
        // clear the stale tail with a second fill of blank cells.
        // `nt_fill_h` emits one buffer entry per call, so the
        // two-step "draw, then erase" pattern costs two entries
        // (~22 bytes) only on the frame the value changes.
        nt_fill_h(2, 1, lives, 0)
        if lives < 5 {
            var blanks: u8 = 5 - lives
            nt_fill_h(2 + lives, 1, blanks, 1)
        }
    }
}

start Main
