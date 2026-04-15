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
