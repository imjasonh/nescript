// jumpjet/constants.ne — layout, gameplay, and tile-index constants.
//
// Every constant fed into rendering, physics, or autopilot lives
// here so a single edit retunes the game. No code, just `const`s.

// ── Playfield layout ─────────────────────────────────────────
//
// Top 16 px (rows 0-1) carry the HUD background row. The sky
// band runs from y = 16 to y = 175, with a 1-row hill stripe at
// the horizon (rows 21) and the ground filling rows 22-29.
const HUD_ROW_Y:        u8 = 0
const SKY_TOP_Y:        u8 = 16
const HORIZON_Y:        u8 = 168     // top of ground band
const GROUND_Y:         u8 = 184     // tanks rest on top of ground

// ── Player jet ───────────────────────────────────────────────
//
// 16×16 metasprite. The jet stays at a fixed screen X centred
// horizontally; only its Y (altitude) changes. Direction is a
// 0/1 flag: 0 = facing right, 1 = facing left.
const JET_X:            u8 = 120
const JET_MIN_Y:        u8 = 24      // ceiling — just below HUD
const JET_MAX_Y:        u8 = 152     // floor — just above horizon

const DIR_RIGHT:        u8 = 0
const DIR_LEFT:         u8 = 1

const JET_VY:           u8 = 1       // px / frame on up/down hold

// ── Enemy planes ─────────────────────────────────────────────
//
// 16×8 metasprite. We spawn three planes at three altitudes
// (high / mid / low). Each plane has a heading; planes that
// reach the screen edge wrap to the opposite side at a fresh y.
const MAX_PLANES:       u8 = 3
const PLANE_W:          u8 = 16
const PLANE_H:          u8 = 8
const PLANE_VX:         u8 = 1

// ── Tanks ────────────────────────────────────────────────────
//
// 16×8 metasprite. Tanks roll along the ground at slow speed.
const MAX_TANKS:        u8 = 2
const TANK_W:           u8 = 16
const TANK_VX:          u8 = 1

// ── Missiles ─────────────────────────────────────────────────
//
// 8×8 sprite. At most two missiles in flight at once; spawning a
// third when the array is full is a no-op.
const MAX_MISSILES:     u8 = 2
const MISSILE_VX:       u8 = 4

// ── Bombs ────────────────────────────────────────────────────
//
// 8×8 sprite. Bombs accelerate downward. At most two in flight.
const MAX_BOMBS:        u8 = 2
const BOMB_VY_INIT:     u8 = 1
const BOMB_VY_CAP:      u8 = 4

// ── Decorative clouds ────────────────────────────────────────
const MAX_CLOUDS:       u8 = 2
const CLOUD_VX:         u8 = 1

// ── Explosions ───────────────────────────────────────────────
const MAX_EXPLOSIONS:   u8 = 2
const EXPLOSION_TTL:    u8 = 12

// ── Lives & score ────────────────────────────────────────────
const START_LIVES:      u8 = 3
const SCORE_DIGITS:     u8 = 5     // 00000 .. 99999

// ── Autopilot timings ────────────────────────────────────────
const TITLE_AUTO_FRAMES:    u8 = 30
// Auto-fire timings are powers of 2 so the periodic checks below
// can use `frame_tick & MASK == 0` instead of `% N` (W0101).
const AUTO_FIRE_MASK:       u8 = 31    // missile every 32 frames (~0.5 s)
const AUTO_BOMB_MASK:       u8 = 63    // bomb every 64 frames (~1 s)
const AUTO_FLIP_MASK:       u8 = 127   // direction flip every 128 frames (~2 s)
const GAMEOVER_LINGER:      u8 = 180   // 3 s on GameOver

// ── HUD positions (sprite-coordinates) ───────────────────────
//
// Score lives in the bg nametable row 1; we paint via nt_set when
// the running counter ticks up. Lives glyph (heart) + count sit
// on the right edge of row 1 in the same way.
const SCORE_X_NT:       u8 = 4       // nt cell column for first score digit
const SCORE_Y_NT:       u8 = 1
const LIVES_HEART_X_NT: u8 = 26
const LIVES_DIGIT_X_NT: u8 = 28
const LIVES_Y_NT:       u8 = 1

// ── Tile indices into Tileset ────────────────────────────────
//
// Tile 0 is the linker-reserved built-in smiley. Every custom
// tile starts at 1.

// 1: blank sky tile (all 0s; renders sub-palette index 0)
const TILE_SKY:         u8 = 1
// 2: solid ground tile
const TILE_GROUND:      u8 = 2
// 3: horizon stripe tile (a row of dirt over a row of grass)
const TILE_HORIZON:     u8 = 3

// 4..29: alphabet A..Z (A = 4, Z = 29)
const TILE_LETTER_BASE: u8 = 4

// 30..39: digits 0..9 (0 = 30, 9 = 39)
const TILE_DIGIT_BASE:  u8 = 30

// 40..43: jet facing right (2×2: TL, TR, BL, BR)
const TILE_JET_R_TL:    u8 = 40
const TILE_JET_R_TR:    u8 = 41
const TILE_JET_R_BL:    u8 = 42
const TILE_JET_R_BR:    u8 = 43

// 44..47: jet facing left (2×2)
const TILE_JET_L_TL:    u8 = 44
const TILE_JET_L_TR:    u8 = 45
const TILE_JET_L_BL:    u8 = 46
const TILE_JET_L_BR:    u8 = 47

// 48..49: enemy plane facing right (left half, right half)
const TILE_PLANE_R_L:   u8 = 48
const TILE_PLANE_R_R:   u8 = 49

// 50..51: enemy plane facing left
const TILE_PLANE_L_L:   u8 = 50
const TILE_PLANE_L_R:   u8 = 51

// 52..53: tank (left half, right half)
const TILE_TANK_L:      u8 = 52
const TILE_TANK_R:      u8 = 53

// 54: missile facing right
const TILE_MISSILE_R:   u8 = 54
// 55: missile facing left
const TILE_MISSILE_L:   u8 = 55
// 56: bomb (falling)
const TILE_BOMB:        u8 = 56
// 57: explosion / muzzle flash
const TILE_EXPLOSION:   u8 = 57
// 58: heart (HUD lives marker)
const TILE_HEART:       u8 = 58
// 59..60: cloud (left half, right half)
const TILE_CLOUD_L:     u8 = 59
const TILE_CLOUD_R:     u8 = 60
