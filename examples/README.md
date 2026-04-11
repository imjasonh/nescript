# NEScript Examples

## Quick Start

### 1. Build the compiler

```
cargo build --release
```

### 2. Compile an example

```
cargo run -- build examples/hello_sprite.ne
```

This produces `examples/hello_sprite.nes` — a valid iNES ROM file.

### 3. Run in an emulator

Open the `.nes` file in any NES emulator:

- **[Mesen](https://www.mesen.ca/)** (recommended — best debugging support)
- **[FCEUX](https://fceux.com/)**
- **[Nestopia](http://nestopia.sourceforge.net/)**

Use the d-pad (arrow keys in most emulators) to move the sprite.

## Examples

| File | Description |
|------|-------------|
| `hello_sprite.ne` | Move a smiley face with the d-pad |
| `bouncing_ball.ne` | A sprite that bounces around the screen automatically |

## What you'll see

The ROM displays a small 8x8 smiley-face sprite. This is a default tile built
into the compiler's CHR data. In `hello_sprite`, you control it with the d-pad.
In `bouncing_ball`, it moves on its own and bounces off the screen edges.

### About sprite names

In Milestone 1, the name in `draw Smiley at: (x, y)` is parsed but not
resolved to a specific tile — all draws use CHR tile 0 (the built-in smiley).
The `draw` syntax is forward-compatible: when `sprite` declarations and the
asset pipeline arrive in M3, names like `Smiley` will reference actual
sprite definitions with custom tile data from PNGs.

## Emulator controls

| NES Button | Typical Key |
|------------|-------------|
| D-pad      | Arrow keys  |
| A          | Z           |
| B          | X           |
| Start      | Enter       |
| Select     | Right Shift |

(Exact mappings vary by emulator — check your emulator's input settings.)

## Compiler commands

```
# Compile to ROM
cargo run -- build examples/hello_sprite.ne

# Compile with custom output path
cargo run -- build examples/hello_sprite.ne --output my_game.nes

# Type-check only (no ROM output)
cargo run -- check examples/hello_sprite.ne
```
