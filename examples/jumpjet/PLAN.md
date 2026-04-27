# JumpJet — Implementation Plan

A NEScript port of JumpJet (Monte Variakojis / Montsoft, 1990) — a
Defender-meets-Scramble side-scrolling shooter where you pilot a
Harrier-like VTOL jet, shooting enemy planes with missiles and
bombing tanks on the ground.

This document is the living plan; check items off as they ship and
record any mid-flight changes in the "Design revisions" section at
the bottom.

---

## 1. Scope & quality bar

A self-contained, headless-harness-friendly port:

- **Title screen** with a big "JUMPJET" logo, blinking "PRESS START"
  marker, music, and an autopilot fall-through to Playing so the
  golden frame at frame 180 captures actual gameplay.
- **Gameplay** with a Harrier jet at fixed screen X / variable Y
  (altitude), enemy planes that drift across the sky at three
  altitudes, two tanks rolling along the ground, missiles that fly
  in the jet's facing direction, gravity-bombs that fall onto
  ground targets, a scoring HUD row at the top, and a 3-life
  pool tracked across deaths.
- **Game-over screen** with the final score, a fanfare, and an
  auto-loop back to Title.
- **Sound**: title music, missile-launch click, bomb-drop swoosh,
  explosion noise burst, victory fanfare on game over.

Out of scope (to keep the port shippable):

- Multiple levels with distinct objectives. The original has four
  similar missions; we ship one. The state machine is structured
  so a future change can splice in additional Playing variants.
- Scrolling background. The world's motion is conveyed by
  sprite-driven cloud / plane / tank movement against a static
  sky-and-ground backdrop. This sidesteps sprite-0 split
  complexity and the harness still reads as a flying jet at frame
  180.

---

## 2. File layout

```
examples/jumpjet.ne                  # top-level: game decl, palette, Tileset, background, audio, includes, start
examples/jumpjet/PLAN.md             # this file
examples/jumpjet/constants.ne        # tile indices, layout + gameplay constants
examples/jumpjet/state.ne            # global state vars
examples/jumpjet/render.ne           # draw helpers (jet, plane, tank, missile, bomb, HUD, words)
examples/jumpjet/title_state.ne      # Title state
examples/jumpjet/play_state.ne       # Playing state
examples/jumpjet/gameover_state.ne   # GameOver state
```

---

## 3. Hardware budget

### Sprite budget per frame

Every sprite uses sp0 (codegen hardwires sp0 in v0.1). The 4
shared colours are **transparent / white / red / lt_gray** —
enough to render a gray jet body with white highlights, red enemy
planes, gray tanks, white missiles, a red heart, and a white-on-
sky alphabet.

| Entity | Qty | Sprites | Notes |
|--------|-----|---------|-------|
| Score digits (HUD) | 5 | 5 | top-left of screen, sprite-rendered for sp0 chrome |
| Heart + lives digit | 2 | 2 | top-right |
| Player jet (16×16) | 1 | 4 | 2×2 metasprite |
| Enemy planes (16×8) | 3 | 6 | 1×2 metasprite each |
| Tanks (16×8) | 2 | 4 | 1×2 metasprite each |
| Missiles | ≤2 | ≤2 | 8×8 single tile each |
| Bombs | ≤2 | ≤2 | 8×8 single tile each |
| Decorative clouds | 2 | 4 | 1×2 metasprite each, drift across sky |
| Explosion FX | ≤2 | ≤2 | flashes briefly on a kill |
| **Total worst-case** | | **≈31** | safely under 64 OAM slots |

**8-per-scanline** worst case: HUD digits at y=8 with the heart at
the same y is 7 sprites — fits. Enemy plane band overlap is
mitigated by spawning enemies at three distinct y values
(48 / 96 / 128); even with all three on screen the worst overlap
is 4 sprites per scanline.

### Tile budget (NROM 8 KB CHR = 256 tiles)

| Group | Tiles | Purpose |
|-------|-------|---------|
| Sky / blank | 1 | bg fill (uses bg0 universal) |
| Ground | 1 | bg fill (uses bg1) |
| Hill-stripe (mid-band) | 1 | bg horizon stripe |
| Cloud-L / Cloud-R (sprite) | 2 | drifting bg motion |
| Alphabet A-Z | 26 | title, GAME OVER, words |
| Digits 0-9 | 10 | score, lives |
| Jet right (2×2) | 4 | facing right metasprite |
| Jet left (2×2) | 4 | facing left metasprite |
| Plane right (2×1) | 2 | facing right enemy |
| Plane left (2×1) | 2 | facing left enemy |
| Tank (2×1) | 2 | ground enemy |
| Missile right | 1 | sprite |
| Missile left | 1 | sprite |
| Bomb | 1 | sprite |
| Explosion | 1 | sprite |
| Heart | 1 | sprite (HUD lives) |
| **Total** | **~60** | leaves ~195 free |

### RAM budget

| Group | Bytes | Notes |
|-------|-------|-------|
| Globals (score, lives, frame_tick, jet_y, jet_dir, …) | ~16 | |
| Enemy plane array (x, y, dir, alive) × 3 | 12 | |
| Tank array (x, alive) × 2 | 4 | |
| Missile array (x, y, dir, alive) × 2 | 8 | |
| Bomb array (x, y, vy, alive) × 2 | 8 | |
| Cloud array (x, y) × 2 | 4 | |
| Explosion array (x, y, ttl) × 2 | 6 | |
| HUD shadow (last_score, last_lives) | 2 | |
| **Total** | **≈60** | well under the 1700-byte general RAM pool |

---

## 4. Controls & autopilot

| Button | Effect |
|--------|--------|
| D-pad ↑ | Climb (jet_y -= 1 per frame, clamped) |
| D-pad ↓ | Dive (jet_y += 1 per frame, clamped) |
| D-pad ← | Face left; world entities scroll right |
| D-pad → | Face right; world entities scroll left |
| A | Fire missile in facing direction |
| B | Drop bomb (always falls down with gravity) |
| Start | (Title) confirm; (GameOver) retry |

The headless emulator harness presses no buttons, so an autopilot
keeps the action visible at frame 180:

- **Title** auto-transitions to Playing after 30 frames.
- **Playing**:
  - jet altitude oscillates sinusoidally driven by `frame_tick`
  - facing direction flips every 90 frames
  - a missile is auto-spawned every 24 frames
  - a bomb is auto-dropped every 48 frames

A human player overrides the autopilot at any time — the autopilot
only fires when the corresponding inputs are not pressed.

---

## 5. State machine

```
Title  → Playing → GameOver → Title
```

`Playing` is a single linear loop (no inner phase machine — the
gameplay is symmetric every frame). Lives are decremented on a
fatal collision; when `lives` hits 0, transition to GameOver.

---

## 6. Audio

| Name | Type | Use |
|------|------|-----|
| `Launch` | sfx pulse 1 | missile fire — sharp ascending blip |
| `Drop` | sfx pulse 1 | bomb drop — descending swoosh |
| `Boom` | sfx noise | explosion on a kill |
| `TitleMusic` | music pulse 2 | brisk loop on Title + Playing |
| builtin `fanfare` | music pulse 2 | one-shot on GameOver |

---

## 7. Implementation steps

1. Skeleton: top-level + every includes/file with at least a stub
   so the program compiles cleanly.
2. Tileset: sky/ground/hill, alphabet, digits, jet (R/L), plane
   (R/L), tank, missile (R/L), bomb, explosion, heart, clouds.
3. Background: 32×30 nametable with HUD row, sky band, hill
   stripe, ground band; palette_map covers each band.
4. Render helpers: draw_letter, draw_digit, draw_count, words
   (JUMPJET, PRESS START, GAME OVER, SCORE, LIVES), draw_jet,
   draw_plane, draw_tank, draw_missile, draw_bomb, draw_clouds,
   draw_hud.
5. State init: globals, enemy plane spawn positions, tank spawn
   positions, cloud start positions.
6. Title state: blinking PRESS START, music, auto-transition.
7. Playing state: input + autopilot, jet physics, missile/bomb
   spawn + step + collision, plane/tank step, cloud step,
   explosion fx, HUD updates, life loss / GameOver transition.
8. GameOver state: GAME OVER text, final score, fanfare,
   auto-return to Title.
9. Capture goldens, eyeball-verify, confirm 23/23 ROMs match.
10. Update top-level README.md + examples/README.md tables.

---

## 8. Design revisions

(populated as the build progresses)
