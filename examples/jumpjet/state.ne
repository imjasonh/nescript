// jumpjet/state.ne — global game state.
//
// Variables that survive across state transitions live here.
// Per-state scratch (animation timers, etc.) lives in the state
// blocks themselves so the analyzer can overlay them.

// ── Frame & autopilot bookkeeping ────────────────────────────
var frame_tick:    u8 = 0     // free-running frame counter, used for autopilot waveforms

// ── Score & lives ────────────────────────────────────────────
//
// Score is stored as a u16 internally so 5 digits (max 99999) fit
// without an array. The HUD draws it as five sprite digits via
// repeated /10 + %10 — slow but only runs when the score actually
// moves.
var score:         u16 = 0
var lives:         u8 = 3

// ── Player jet ───────────────────────────────────────────────
var jet_y:         u8 = 80      // altitude (top-left of the 16×16 metasprite)
var jet_dir:       u8 = 0       // 0 = right, 1 = left

// ── Enemy planes ─────────────────────────────────────────────
var plane_x:       u8[3] = [0, 0, 0]
var plane_y:       u8[3] = [0, 0, 0]
var plane_dir:     u8[3] = [0, 0, 0]
var plane_alive:   u8[3] = [0, 0, 0]

// ── Tanks ────────────────────────────────────────────────────
var tank_x:        u8[2] = [0, 0]
var tank_alive:    u8[2] = [0, 0]

// ── Missiles ─────────────────────────────────────────────────
var missile_x:     u8[2] = [0, 0]
var missile_y:     u8[2] = [0, 0]
var missile_dir:   u8[2] = [0, 0]
var missile_alive: u8[2] = [0, 0]

// ── Bombs ────────────────────────────────────────────────────
var bomb_x:        u8[2] = [0, 0]
var bomb_y:        u8[2] = [0, 0]
var bomb_vy:       u8[2] = [0, 0]
var bomb_alive:    u8[2] = [0, 0]

// ── Decorative clouds ────────────────────────────────────────
var cloud_x:       u8[2] = [40, 200]
var cloud_y:       u8[2] = [32, 56]

// ── Explosions ───────────────────────────────────────────────
var exp_x:         u8[2] = [0, 0]
var exp_y:         u8[2] = [0, 0]
var exp_ttl:       u8[2] = [0, 0]
