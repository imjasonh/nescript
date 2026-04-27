// jumpjet/play_state.ne — the gameplay loop.
//
// The jet sits at a fixed screen X with a variable Y (altitude).
// Direction of travel is conveyed by sprites that move across the
// screen: planes, tanks, and clouds drift in the opposite
// direction of the jet's heading, which sells the illusion of
// flight without a scrolling background. Missiles fly forward
// in the jet's facing direction; bombs fall straight down.
//
// The headless harness presses no buttons, so an autopilot drives
// the action: altitude oscillates with frame_tick, direction
// flips every AUTO_FLIP_FRAMES frames, missiles auto-spawn every
// AUTO_FIRE_FRAMES, bombs every AUTO_BOMB_FRAMES.

// ── Spawn helpers ────────────────────────────────────────────

// Spawn a missile in the first free slot. Inherits the jet's
// facing direction and altitude.
fun fire_missile() {
    var i: u8 = 0
    while i < MAX_MISSILES {
        if missile_alive[i] == 0 {
            missile_alive[i] = 1
            missile_dir[i] = jet_dir
            missile_y[i] = jet_y + 4
            // Spawn just outside the jet's nose so the missile
            // doesn't render under the jet body.
            if jet_dir == DIR_RIGHT {
                missile_x[i] = JET_X + 16
            } else {
                missile_x[i] = JET_X - 8
            }
            play Launch
            return
        }
        i += 1
    }
}

// Drop a bomb in the first free slot. Always falls straight down
// from underneath the jet.
fun drop_bomb() {
    var i: u8 = 0
    while i < MAX_BOMBS {
        if bomb_alive[i] == 0 {
            bomb_alive[i] = 1
            bomb_x[i] = JET_X + 4
            bomb_y[i] = jet_y + 16
            bomb_vy[i] = BOMB_VY_INIT
            play Drop
            return
        }
        i += 1
    }
}

// Spawn an explosion at (x, y) in the first available slot.
// Drives the explosion render via exp_ttl > 0.
fun spawn_explosion(x: u8, y: u8) {
    var i: u8 = 0
    while i < MAX_EXPLOSIONS {
        if exp_ttl[i] == 0 {
            exp_x[i] = x
            exp_y[i] = y
            exp_ttl[i] = EXPLOSION_TTL
            play Boom
            return
        }
        i += 1
    }
}

// ── Per-entity step functions ────────────────────────────────

// Step every active missile by MISSILE_VX in its heading. When
// a missile leaves the screen its slot is freed.
fun step_missiles() {
    var i: u8 = 0
    while i < MAX_MISSILES {
        if missile_alive[i] == 1 {
            if missile_dir[i] == DIR_RIGHT {
                missile_x[i] += MISSILE_VX
                if missile_x[i] >= 248 {
                    missile_alive[i] = 0
                }
            } else {
                if missile_x[i] >= MISSILE_VX {
                    missile_x[i] -= MISSILE_VX
                } else {
                    missile_alive[i] = 0
                }
            }
        }
        i += 1
    }
}

// Step every bomb downward, accelerating by 1 px/frame up to
// BOMB_VY_CAP. Bombs that hit the ground despawn (with no FX
// here — tank-hit FX are produced by the collision pass).
fun step_bombs() {
    var i: u8 = 0
    while i < MAX_BOMBS {
        if bomb_alive[i] == 1 {
            bomb_y[i] += bomb_vy[i]
            if bomb_vy[i] < BOMB_VY_CAP {
                bomb_vy[i] += 1
            }
            if bomb_y[i] >= GROUND_Y {
                bomb_alive[i] = 0
            }
        }
        i += 1
    }
}

// Step every active enemy plane in its facing direction. Planes
// that exit the screen wrap to the opposite edge at a fresh y so
// the screen always has something to shoot at.
fun step_planes() {
    var i: u8 = 0
    while i < MAX_PLANES {
        if plane_alive[i] == 1 {
            if plane_dir[i] == DIR_RIGHT {
                plane_x[i] += PLANE_VX
                if plane_x[i] >= 240 {
                    plane_x[i] = 0
                }
            } else {
                if plane_x[i] >= PLANE_VX {
                    plane_x[i] -= PLANE_VX
                } else {
                    plane_x[i] = 240
                }
            }
        }
        i += 1
    }
}

// Tanks roll along the ground in a fixed direction (left in this
// build) and wrap.
fun step_tanks() {
    var i: u8 = 0
    while i < MAX_TANKS {
        if tank_alive[i] == 1 {
            if tank_x[i] >= TANK_VX {
                tank_x[i] -= TANK_VX
            } else {
                tank_x[i] = 240
            }
        }
        i += 1
    }
}

// Clouds drift slowly opposite the jet's heading.
fun step_clouds() {
    var i: u8 = 0
    while i < MAX_CLOUDS {
        if jet_dir == DIR_RIGHT {
            // Jet flies right → world (clouds) appears to move left.
            if cloud_x[i] >= CLOUD_VX {
                cloud_x[i] -= CLOUD_VX
            } else {
                cloud_x[i] = 240
            }
        } else {
            cloud_x[i] += CLOUD_VX
            if cloud_x[i] >= 240 {
                cloud_x[i] = 0
            }
        }
        i += 1
    }
}

// Tick every active explosion ttl down to zero. The renderer
// keys off ttl > 0.
fun step_explosions() {
    var i: u8 = 0
    while i < MAX_EXPLOSIONS {
        if exp_ttl[i] > 0 {
            exp_ttl[i] -= 1
        }
        i += 1
    }
}

// ── Collisions ───────────────────────────────────────────────

// Missile vs. plane: 16×8 plane AABB against the 8×8 missile
// hitbox. On a hit, both are despawned, an explosion fires at
// the plane's centre, and the score bumps by 100. The plane is
// respawned at the opposite screen edge with a fresh dir/y so
// the action stays continuous.
fun missile_vs_planes() {
    var m: u8 = 0
    while m < MAX_MISSILES {
        if missile_alive[m] == 1 {
            var mx: u8 = missile_x[m]
            var my: u8 = missile_y[m]
            var p: u8 = 0
            while p < MAX_PLANES {
                if plane_alive[p] == 1 {
                    var px: u8 = plane_x[p]
                    var py: u8 = plane_y[p]
                    // Horizontal overlap: missile x ∈ [px-8, px+16)
                    if mx + 8 > px {
                        if mx < px + PLANE_W {
                            // Vertical overlap: missile y ∈ [py-8, py+8)
                            if my + 8 > py {
                                if my < py + PLANE_H {
                                    missile_alive[m] = 0
                                    plane_alive[p] = 0
                                    spawn_explosion(px, py)
                                    score += 100
                                }
                            }
                        }
                    }
                }
                p += 1
            }
        }
        m += 1
    }
}

// Bomb vs. tank: bomb AABB against the tank AABB at GROUND_Y. On
// a hit, the bomb despawns, the tank explodes, score bumps by
// 200 (tanks worth more than planes since bombing is harder).
// Tank respawns at a wrap edge so the player still has targets.
fun bomb_vs_tanks() {
    var b: u8 = 0
    while b < MAX_BOMBS {
        if bomb_alive[b] == 1 {
            var bx: u8 = bomb_x[b]
            var by: u8 = bomb_y[b]
            // Bombs only hit when their y is in the ground band
            if by + 8 > GROUND_Y {
                var t: u8 = 0
                while t < MAX_TANKS {
                    if tank_alive[t] == 1 {
                        var tx: u8 = tank_x[t]
                        if bx + 8 > tx {
                            if bx < tx + TANK_W {
                                bomb_alive[b] = 0
                                tank_alive[t] = 0
                                spawn_explosion(tx, GROUND_Y)
                                score += 200
                            }
                        }
                    }
                    t += 1
                }
            }
        }
        b += 1
    }
}

// Plane vs. jet: a plane that overlaps the jet's 16×16 hitbox is
// fatal. The jet flashes (via lives--), the plane despawns and
// respawns elsewhere so the player isn't immediately killed
// again, and an explosion fires at the jet's location.
fun plane_vs_jet() {
    var p: u8 = 0
    while p < MAX_PLANES {
        if plane_alive[p] == 1 {
            var px: u8 = plane_x[p]
            var py: u8 = plane_y[p]
            // Player AABB at (JET_X, jet_y, 16, 16) vs plane (px, py, 16, 8)
            if px + PLANE_W > JET_X {
                if px < JET_X + 16 {
                    if py + PLANE_H > jet_y {
                        if py < jet_y + 16 {
                            plane_alive[p] = 0
                            spawn_explosion(JET_X, jet_y)
                            if lives > 0 {
                                lives -= 1
                            }
                        }
                    }
                }
            }
        }
        p += 1
    }
}

// ── Respawn helpers ──────────────────────────────────────────

// Re-seed any dead plane / tank slot at the opposite edge of the
// screen so the action keeps flowing. Called once a frame after
// stepping + collisions.
fun respawn_dead() {
    var i: u8 = 0
    while i < MAX_PLANES {
        if plane_alive[i] == 0 {
            plane_alive[i] = 1
            // Alternate spawn edge by the slot index parity.
            if (i & 1) == 0 {
                plane_x[i] = 0
                plane_dir[i] = DIR_RIGHT
            } else {
                plane_x[i] = 232
                plane_dir[i] = DIR_LEFT
            }
            // Stagger plane Y across three altitude bands. Each
            // band sits inside the jet's autopilot altitude wave
            // (y = 56..119) at a phase the auto-fire pulses
            // actually visit, so missiles produce visible kills
            // in the headless harness golden.
            if i == 0 {
                plane_y[i] = 64
            } else if i == 1 {
                plane_y[i] = 88
            } else {
                plane_y[i] = 112
            }
        }
        i += 1
    }
    var t: u8 = 0
    while t < MAX_TANKS {
        if tank_alive[t] == 0 {
            tank_alive[t] = 1
            if (t & 1) == 0 {
                tank_x[t] = 200
            } else {
                tank_x[t] = 60
            }
        }
        t += 1
    }
}

// ── HUD updates ──────────────────────────────────────────────

fun draw_hud() {
    // Top-left score: redraw every frame because draw_score takes
    // ~5 sprite draws and the digits never overlap any other
    // sprite, so the per-frame cost is fixed and small.
    draw_score(8, 8)
    // Top-right lives indicator (heart + digit).
    draw_lives(216, 8)
}

// ── State definition ─────────────────────────────────────────

state Playing {
    on enter {
        // Spawn the initial wave — three planes at three altitudes
        // and two tanks on the ground. We mark every slot dead and
        // immediately call respawn_dead() so the spawn logic lives
        // in exactly one place.
        var i: u8 = 0
        while i < MAX_PLANES {
            plane_alive[i] = 0
            i += 1
        }
        var t: u8 = 0
        while t < MAX_TANKS {
            tank_alive[t] = 0
            t += 1
        }
        var m: u8 = 0
        while m < MAX_MISSILES {
            missile_alive[m] = 0
            m += 1
        }
        var b: u8 = 0
        while b < MAX_BOMBS {
            bomb_alive[b] = 0
            b += 1
        }
        var e: u8 = 0
        while e < MAX_EXPLOSIONS {
            exp_ttl[e] = 0
            e += 1
        }
        respawn_dead()

        jet_y = 80
        jet_dir = DIR_RIGHT
        frame_tick = 0
        // `lives` and `score` carry over from Title's `on enter`.
        start_music TitleMusic
    }

    on frame {
        frame_tick += 1

        // ── Input + autopilot ─────────────────────────────────
        // Direction: human override beats the autopilot.
        if button.left {
            jet_dir = DIR_LEFT
        } else if button.right {
            jet_dir = DIR_RIGHT
        } else {
            // Autopilot flips heading every AUTO_FLIP_MASK+1 frames.
            if (frame_tick & AUTO_FLIP_MASK) == 0 {
                if jet_dir == DIR_RIGHT {
                    jet_dir = DIR_LEFT
                } else {
                    jet_dir = DIR_RIGHT
                }
            }
        }

        // Altitude: human up/down or autopilot oscillation.
        var any_alt: u8 = 0
        if button.up {
            if jet_y > JET_MIN_Y {
                jet_y -= JET_VY
            }
            any_alt = 1
        }
        if button.down {
            if jet_y < JET_MAX_Y {
                jet_y += JET_VY
            }
            any_alt = 1
        }
        if any_alt == 0 {
            // Autopilot: a triangular waveform on frame_tick keeps
            // the jet bobbing between altitudes 56 and 120 over a
            // ~2-second period.
            var phase: u8 = frame_tick & 0x7F
            if phase < 64 {
                jet_y = 56 + phase
            } else {
                jet_y = 56 + (127 - phase)
            }
        }

        // Fire missile: human or autopilot.
        if button.a {
            fire_missile()
        } else {
            if (frame_tick & AUTO_FIRE_MASK) == 0 {
                fire_missile()
            }
        }

        // Drop bomb: human or autopilot. Offset by 16 so missile
        // and bomb autopilot pulses never coincide on the same
        // frame.
        if button.b {
            drop_bomb()
        } else {
            if (frame_tick & AUTO_BOMB_MASK) == 16 {
                drop_bomb()
            }
        }

        // ── Step world ────────────────────────────────────────
        step_missiles()
        step_bombs()
        step_planes()
        step_tanks()
        step_clouds()
        step_explosions()

        // ── Collisions ────────────────────────────────────────
        missile_vs_planes()
        bomb_vs_tanks()
        plane_vs_jet()

        // ── Respawn anything that died this frame ─────────────
        respawn_dead()

        // ── Draw ──────────────────────────────────────────────
        draw_clouds()
        draw_planes()
        draw_tanks()
        draw_missiles()
        draw_bombs()
        draw_jet()
        draw_explosions()
        draw_hud()

        // Death check.
        if lives == 0 {
            transition GameOver
        }
    }
}
