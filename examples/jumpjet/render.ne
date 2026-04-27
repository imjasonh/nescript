// jumpjet/render.ne — drawing + HUD helpers.
//
// Every on-screen entity (jet, planes, tanks, missiles, bombs,
// clouds, explosions, HUD glyphs) goes through one of the
// helpers in this file, so each state's `on frame` reads as a
// short orchestration script.

// ── Letter / digit helpers ───────────────────────────────────
fun draw_letter(x: u8, y: u8, ch: u8) {
    draw Tileset at: (x, y) frame: TILE_LETTER_BASE + ch
}

fun draw_digit(x: u8, y: u8, d: u8) {
    draw Tileset at: (x, y) frame: TILE_DIGIT_BASE + d
}

// ── Words ────────────────────────────────────────────────────
//
// Letter indices are 0-based off TILE_LETTER_BASE: A=0, B=1,
// ..., Z=25.

// "JUMPJET" — title banner (7 letters, 56 px wide)
fun draw_word_jumpjet(x: u8, y: u8) {
    draw_letter(x,      y, 9)   // J
    draw_letter(x + 8,  y, 20)  // U
    draw_letter(x + 16, y, 12)  // M
    draw_letter(x + 24, y, 15)  // P
    draw_letter(x + 32, y, 9)   // J
    draw_letter(x + 40, y, 4)   // E
    draw_letter(x + 48, y, 19)  // T
}

// "PRESS" — used on the title's blinking prompt
fun draw_word_press(x: u8, y: u8) {
    draw_letter(x,      y, 15)  // P
    draw_letter(x + 8,  y, 17)  // R
    draw_letter(x + 16, y, 4)   // E
    draw_letter(x + 24, y, 18)  // S
    draw_letter(x + 32, y, 18)  // S
}

// "START"
fun draw_word_start(x: u8, y: u8) {
    draw_letter(x,      y, 18)  // S
    draw_letter(x + 8,  y, 19)  // T
    draw_letter(x + 16, y, 0)   // A
    draw_letter(x + 24, y, 17)  // R
    draw_letter(x + 32, y, 19)  // T
}

// "GAME"
fun draw_word_game(x: u8, y: u8) {
    draw_letter(x,      y, 6)   // G
    draw_letter(x + 8,  y, 0)   // A
    draw_letter(x + 16, y, 12)  // M
    draw_letter(x + 24, y, 4)   // E
}

// "OVER"
fun draw_word_over(x: u8, y: u8) {
    draw_letter(x,      y, 14)  // O
    draw_letter(x + 8,  y, 21)  // V
    draw_letter(x + 16, y, 4)   // E
    draw_letter(x + 24, y, 17)  // R
}

// "SCORE"
fun draw_word_score(x: u8, y: u8) {
    draw_letter(x,      y, 18)  // S
    draw_letter(x + 8,  y, 2)   // C
    draw_letter(x + 16, y, 14)  // O
    draw_letter(x + 24, y, 17)  // R
    draw_letter(x + 32, y, 4)   // E
}

// ── Score & lives HUD ────────────────────────────────────────
//
// Score is rendered as five sprite digits at the top-left of the
// screen, fed by repeated /10 + %10 splits. Cheap because it
// only runs when the score actually changes.
//
// Five-digit score split (no /10000 / /1000 because both are
// expensive 16-bit divides). We pull the highest digits first
// using subtract-and-count loops bounded by the maximum value
// (99 999) so the loop count is tight.
fun draw_score(x: u8, y: u8) {
    var n: u16 = score
    var d4: u8 = 0
    while n >= 10000 {
        n -= 10000
        d4 += 1
    }
    var d3: u8 = 0
    while n >= 1000 {
        n -= 1000
        d3 += 1
    }
    var d2: u8 = 0
    while n >= 100 {
        n -= 100
        d2 += 1
    }
    var lo: u8 = n as u8
    var d1: u8 = 0
    while lo >= 10 {
        lo -= 10
        d1 += 1
    }
    draw_digit(x,      y, d4)
    draw_digit(x + 8,  y, d3)
    draw_digit(x + 16, y, d2)
    draw_digit(x + 24, y, d1)
    draw_digit(x + 32, y, lo)
}

fun draw_lives(x: u8, y: u8) {
    draw Tileset at: (x, y) frame: TILE_HEART
    draw_digit(x + 12, y, lives)
}

// ── Sky decorations ──────────────────────────────────────────
fun draw_clouds() {
    var i: u8 = 0
    while i < MAX_CLOUDS {
        draw Tileset at: (cloud_x[i],     cloud_y[i]) frame: TILE_CLOUD_L
        draw Tileset at: (cloud_x[i] + 8, cloud_y[i]) frame: TILE_CLOUD_R
        i += 1
    }
}

// ── Player jet (16×16 metasprite) ────────────────────────────
fun draw_jet() {
    if jet_dir == DIR_RIGHT {
        draw Tileset at: (JET_X,     jet_y    ) frame: TILE_JET_R_TL
        draw Tileset at: (JET_X + 8, jet_y    ) frame: TILE_JET_R_TR
        draw Tileset at: (JET_X,     jet_y + 8) frame: TILE_JET_R_BL
        draw Tileset at: (JET_X + 8, jet_y + 8) frame: TILE_JET_R_BR
    } else {
        draw Tileset at: (JET_X,     jet_y    ) frame: TILE_JET_L_TL
        draw Tileset at: (JET_X + 8, jet_y    ) frame: TILE_JET_L_TR
        draw Tileset at: (JET_X,     jet_y + 8) frame: TILE_JET_L_BL
        draw Tileset at: (JET_X + 8, jet_y + 8) frame: TILE_JET_L_BR
    }
}

// ── Enemies ──────────────────────────────────────────────────
fun draw_planes() {
    var i: u8 = 0
    while i < MAX_PLANES {
        if plane_alive[i] == 1 {
            if plane_dir[i] == DIR_RIGHT {
                draw Tileset at: (plane_x[i],     plane_y[i]) frame: TILE_PLANE_R_L
                draw Tileset at: (plane_x[i] + 8, plane_y[i]) frame: TILE_PLANE_R_R
            } else {
                draw Tileset at: (plane_x[i],     plane_y[i]) frame: TILE_PLANE_L_L
                draw Tileset at: (plane_x[i] + 8, plane_y[i]) frame: TILE_PLANE_L_R
            }
        }
        i += 1
    }
}

fun draw_tanks() {
    var i: u8 = 0
    while i < MAX_TANKS {
        if tank_alive[i] == 1 {
            draw Tileset at: (tank_x[i],     GROUND_Y) frame: TILE_TANK_L
            draw Tileset at: (tank_x[i] + 8, GROUND_Y) frame: TILE_TANK_R
        }
        i += 1
    }
}

// ── Projectiles ──────────────────────────────────────────────
fun draw_missiles() {
    var i: u8 = 0
    while i < MAX_MISSILES {
        if missile_alive[i] == 1 {
            if missile_dir[i] == DIR_RIGHT {
                draw Tileset at: (missile_x[i], missile_y[i]) frame: TILE_MISSILE_R
            } else {
                draw Tileset at: (missile_x[i], missile_y[i]) frame: TILE_MISSILE_L
            }
        }
        i += 1
    }
}

fun draw_bombs() {
    var i: u8 = 0
    while i < MAX_BOMBS {
        if bomb_alive[i] == 1 {
            draw Tileset at: (bomb_x[i], bomb_y[i]) frame: TILE_BOMB
        }
        i += 1
    }
}

fun draw_explosions() {
    var i: u8 = 0
    while i < MAX_EXPLOSIONS {
        if exp_ttl[i] > 0 {
            draw Tileset at: (exp_x[i], exp_y[i]) frame: TILE_EXPLOSION
        }
        i += 1
    }
}
