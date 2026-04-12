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
| `sprites_and_palettes.ne` | sprites, palettes, scroll, cast | Inline CHR data, palette switching, type casting |
| `mmc1_banked.ne` | MMC1, banks, multiply | Banked mapper with software multiply |

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
