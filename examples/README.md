# NEScript Examples

## Quick Start

```bash
# Build the compiler
cargo build --release

# Compile all examples
for f in examples/*.ne; do cargo run -- build "$f"; done

# Or compile one
cargo run -- build examples/hello_sprite.ne
```

Open any `.nes` file in an NES emulator ([Mesen](https://www.mesen.ca/), [FCEUX](https://fceux.com/), etc.)

## Examples

| File | Features | Description |
|------|----------|-------------|
| `hello_sprite.ne` | input, draw | Move a sprite with the d-pad |
| `bouncing_ball.ne` | if/else, variables | Auto-bouncing sprite with edge detection |
| `coin_cavern.ne` | states, functions, constants | 3-state game with gravity and coin collection |
| `arrays_and_functions.ne` | arrays, functions, while | Enemy array with collision detection |
| `state_machine.ne` | on enter/exit, transitions | Multi-state flow with timers |
| `sprites_and_palettes.ne` | sprites, scroll, cast | Inline CHR data, PPU scroll writes, type casting |
| `mmc1_banked.ne` | MMC1, banks, multiply | Banked mapper with software multiply |
| `uxrom_user_banked.ne` | UxROM, `bank Foo { fun ... }`, cross-bank trampoline | First example to put real user code inside a switchable bank. The animation step lives in `bank Extras` and is invoked from the fixed-bank state handler via a generated `__tramp_step_animation` stub that selects bank 0, JSRs the body, then restores the fixed bank before returning. |
| `uxrom_banked_to_banked.ne` | UxROM, banked → banked cross-bank call | Two `bank Foo { fun ... }` blocks: `step` lives in bank Logic and calls `clamp` in bank Helpers. The trampoline uses `ZP_BANK_CURRENT + PHA/PLA` to save and restore the caller's bank, so the same per-callee stub works whether the caller is in the fixed bank or another switchable bank. |
| `palette_and_background.ne` | palette, background, set_palette, load_background | Reset-time initial load plus vblank-safe runtime swaps |
| `auto_chr_background.ne` | `background @nametable(...)` with auto-CHR | First example to use the `@nametable("file.png")` shortcut without supplying any matching CHR data. The resolver dedupes the PNG's 8×8 cells, encodes them via the same brightness-bucketing the sprite CHR encoder uses, and slots them into CHR ROM at the next free tile slot. The committed `auto_chr_bg.png` is a 256×240 grayscale gradient that exercises ~50 unique tiles. |
| `friendly_assets.ne` | named colours, grouped palette, pixel art, tilemap+legend, palette_map, scalar sfx pitch, note-name music | Exercises every "friendlier" asset syntax at once — the `palette` uses `bg0..sp3` + a shared `universal:`, the sprite is authored as ASCII pixel art, the background uses a `legend { ... } + map:` tilemap with a `palette_map:` for attributes, the sfx uses a scalar `pitch:` + `envelope:` alias, and the music uses note names (`C4, E4 40, rest 10`) with a `tempo:` default. |
| `noise_triangle_sfx.ne` | `channel: noise`, `channel: triangle` on `sfx` blocks | Demonstrates the noise and triangle sfx channels. Declares one noise burst and one triangle bass note, plays each on a timer so the emulator harness captures both the pixel output and the APU state. |
| `sfx_pitch_envelope.ne` | varying-pitch pulse SFX | A 16-frame frequency sweep written as a per-frame `pitch:` array on a Pulse-1 sfx. The compiler emits a separate `__sfx_pitch_<name>` blob and gates the audio tick's pitch update path on the `__sfx_pitch_used` marker, so programs that stick to the scalar `pitch:` form still get byte-identical ROM output. |
| `metasprite_demo.ne` | declarative multi-tile sprites | A 16×16 hero sprite split into a `metasprite Hero { sprite: Hero16, dx: [...], dy: [...], frame: [...] }` declaration. `draw Hero at: (px, py)` then expands to one `DrawSprite` op per tile in the IR lowering, each with its dx/dy added to the user's anchor point and the frame offset by the underlying sprite's base tile. The codegen needs no metasprite-specific support — it sees N regular draws and the OAM cursor allocator handles the slots. |
| `nested_structs.ne` | nested struct fields, array struct fields, chained literals | Two `Hero` instances each carry a `Vec2` position and a `u8[4]` inventory. Exercises `hero.pos.x` chained access, `hero.inv[i]` array-field access, and chained struct-literal initializers (`Hero { pos: Vec2 { x: ..., y: ... }, inv: [...] }`). |
| `platformer.ne` | **every subsystem** | End-to-end side-scrolling demo: custom CHR tileset, full 32×30 nametable with per-region attribute palettes, 2×2 metasprite hero with gravity/jump physics, wrap-around horizontal scrolling, stomp-or-die enemy collisions with a live stomp-count HUD, coin pickups, user-declared SFX + music, and a Title → Playing → GameOver state machine with a proximity-based autopilot so the headless harness cycles through stomp, stomp, die, and retry inside six seconds. Regenerate the tile art with `cargo run --bin gen_platformer_tiles`. |
| `sprite_flicker_demo.ne` | `cycle_sprites`, 8-per-scanline hardware limit | Twelve sprites packed onto the same 4-pixel band — two more than the NES's 8-sprites-per-scanline hardware budget. The W0109 analyzer warning fires at compile time, and a `cycle_sprites` call at the end of `on frame` rotates the OAM DMA offset one slot per frame so the PPU drops a *different* sprite each frame. The permanent-dropout failure mode becomes visible flicker, which the eye reconstructs across frames. The classic NES technique used by Gradius, Battletoads, and every shmup that ever existed. |
| `war.ne` | **production-quality card game**, multi-file source layout | A complete port of the card game War, split across `examples/war/*.ne` files and pulled in via `include` directives. Title screen with a 0/1/2-player menu (cursor sprite, blinking PRESS A, brisk 4/4 march on pulse 2), a 50-frame deal animation, a deep `Playing` state with an inner phase machine (`P_WAIT_A`/`P_FLY_A`/.../`P_WAR_BANNER`/`P_WAR_BURY`/`P_CHECK`), card-conserving queue-based decks built on a 200-iteration random-swap shuffle, a "WAR!" tie-break that buries 3+1 face-down cards per player and plays a noise-channel thump per bury, and a victory screen with the builtin fanfare. The first NEScript example to use a top-level file as a thin shell that `include`s ~12 component files; building it surfaced seven compiler bugs across the analyzer, IR lowerer, and codegen that were all fixed on the same branch (see `git log` for details). |
| `pong.ne` | **production-quality Pong**, powerups, multi-ball, multi-file | A complete Pong game split across `examples/pong/*.ne`. CPU VS CPU / 1 PLAYER / 2 PLAYERS title menu with brisk pulse-2 title march and autopilot, smooth ball physics with wall and paddle bouncing, CPU AI that tracks the ball with a reaction lag and dead zone, three powerup types (LONG paddle for 5 hits, FAST ball on next hit, MULTI-ball on next hit spawning 3 balls) that bounce around the field and are caught by paddle AABB overlap, multi-ball scoring (each ball scores a point, round continues until last ball exits), inner phase machine (`P_SERVE`/`P_PLAY`/`P_POINT`), and a "PLAYER N WINS" victory screen with the builtin fanfare. First-to-7 wins. |
| `feature_canary.ne` | **regression canary**, state-locals, uninitialized struct-field writes, u16, arrays, `slow` placement, function returns | A minimal program whose sole job is to paint a green universal backdrop at frame 180 when every memory-affecting language construct round-trips a write through the compiler correctly, and to flip to red if any check fails. Each check writes a distinctive byte through one construct (state-local, uninit struct field, u8/u16 global, array element, `slow`-placed u8, function call return), reads it back, and clears `all_ok` on mismatch. Because the emulator harness compares pixels at frame 180, any compiler regression that silently drops one of these writes turns the committed golden red — the structural counter to the "goldens capture whatever happens, not what should happen" failure mode that let PR #31 survive for a year. |
| `sha256.ne` | **interactive SHA-256**, inline-asm 32-bit primitives, multi-file | A full FIPS 180-4 SHA-256 hasher split across `examples/sha256/*.ne`. An on-screen 5×8 keyboard grid lets the player type up to 16 ASCII characters (`A`..`Z`, `0`..`9`, space, `.`, backspace, enter), and pressing ↵ runs the 48-entry message-schedule expansion + 64-round compression on the NES itself. Every 32-bit primitive (`copy`, `xor`, `and`, `add`, `not`, rotate-right, shift-right) is hand-tuned inline assembly that walks the four little-endian bytes of a word with `LDA {wk},X` / `ADC {wk},Y` chains, so a whole round costs a few thousand cycles. The phased driver runs four schedule steps or four rounds per frame so the full compression finishes well under a second, and the 64-character hex digest renders as sprites in 8 rows of 8 glyphs at the bottom of the screen. The jsnes golden auto-types `"NES"` after 1 s of keyboard idle and captures its hash `AE9145DB5CABC41FE34B54E34AF8881F462362EA20FD8F861B26532FFBB84E0D`. |
| `prng_demo.ne` | `rand8()`, `rand16()`, `seed_rand()` | Exercises the runtime xorshift PRNG end-to-end. Four sprite positions are drawn from fresh `rand8()` draws every frame with a `rand16()` sample mixed in. `seed_rand(0x1234)` pins the initial state so the golden is deterministic. The `__rand_used` marker gates linking of `gen_prng` + the reset-time seed — programs that never call any of the three get zero ROM / cycle overhead. |
| `edge_input_demo.ne` | `p1.button.a.pressed`, `p1.button.b.released` | Demonstrates edge-triggered input. The A-sprite advances exactly once per press transition (holding the button does nothing) and the B-sprite advances on release. Lowering emits `IrOp::ReadInputEdge`, which stores the previous-frame input byte into main RAM and XORs it against the current byte at the read site. The NMI handler snapshots both prev bytes before strobing, gated on the `__edge_input_used` marker. |
| `palette_brightness_demo.ne` | `set_palette_brightness(level)` | Cycles through the 9 brightness levels (0 = blank, 4 = normal, 8 = max emphasis) every 20 frames. Exercises the neslib-style `pal_bright` mapping onto `$2001` PPU mask emphasis bits. The runtime routine `__set_palette_brightness` is spliced in only when user code references the builtin. |
| `axrom_simple.ne` | `mapper: AxROM` (mapper 7) | Single-screen AxROM demo. The linker pads PRG to 32 KB (one blank 16 KB bank plus our 16 KB fixed bank) so emulators that enforce mapper-7's 32 KB page size boot cleanly. Register layout: bit 4 of `$8000` selects single-screen lower / upper nametable. |
| `cnrom_simple.ne` | `mapper: CNROM` (mapper 3) | CNROM demo. Fixed 32 KB PRG, switchable 8 KB CHR. Single-bank CNROM is functionally equivalent to NROM at the PRG level, but the iNES header reports mapper 3 and the runtime writes a CHR bank 0 select at reset. |

## Emulator Controls

| NES Button | Typical Key |
|------------|-------------|
| D-pad      | Arrow keys  |
| A          | Z           |
| B          | X           |
| Start      | Enter       |
| Select     | Right Shift |

## About Sprites

Sprite names in `draw Player at: (x, y)` are parsed and recorded in the AST.
You can define sprites with inline CHR tile data:

```
sprite Player {
    chr: [0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C,
          0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
}
```

If no matching sprite declaration exists, the draw uses the built-in default
tile (a smiley face). See `sprites_and_palettes.ne` for a full example.

## Compiler Commands

```bash
# Compile to ROM
cargo run -- build game.ne

# Custom output path
cargo run -- build game.ne --output my_game.nes

# Type-check only
cargo run -- check game.ne

# View generated 6502 assembly
cargo run -- build game.ne --asm-dump

# Debug mode
cargo run -- build game.ne --debug
```
