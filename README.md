# NEScript

A statically-typed, compiled programming language for NES game development.

NEScript compiles `.ne` source files directly into playable iNES ROM files, with no external assembler or linker dependencies. The compiler handles everything from source text to a ROM you can run in any NES emulator.

## Quick Start

```bash
# Build the compiler
cargo build --release

# Compile an example
cargo run -- build examples/hello_sprite.ne

# Run the output ROM in an emulator
# (produces examples/hello_sprite.nes)
```

## Hello World

```
game "Hello" {
    mapper: NROM
}

var px: u8 = 128
var py: u8 = 120

on frame {
    if button.right { px += 2 }
    if button.left  { px -= 2 }
    if button.down  { py += 2 }
    if button.up    { py -= 2 }

    draw Smiley at: (px, py)
}

start Main
```

## Features

- **Game-aware syntax** -- states, sprites, palettes, and input are first-class constructs
- **Full type system** -- `u8`, `i8`, `u16`, `bool`, fixed-size arrays (`u8[N]`), `enum`, `struct`
- **Rich control flow** -- `if`/`else`, `while`, `for i in 0..N`, `loop`, `match`
- **Functions** -- with parameters, return types, `inline` hint, recursion detection
- **State machines** -- `state` with `on enter`, `on exit`, `on frame`, `on scanline(N)` handlers
- **Compile-time safety** -- call depth limits, recursion detection, type checking, unused-var warnings
- **IR-based optimizer** -- constant folding, dead code elimination, strength reduction, copy propagation, peephole passes
- **Multiple mappers** -- NROM, MMC1, UxROM, MMC3 (including scanline IRQ dispatch)
- **Asset pipeline** -- PNG-to-CHR conversion, palette definitions, inline tile data
- **Inline assembly** -- `asm { ... }` with `{var}` substitution, plus `raw asm { ... }` for verbatim blocks
- **Hardware intrinsics** -- `poke(addr, value)` / `peek(addr)` for direct register access
- **Debug support** -- `--debug` flag, source maps, Mesen-compatible symbol export, `debug.log` / `debug.assert`
- **Compile-time diagnostics** -- `--dump-ir`, `--memory-map`, `--call-graph` flags
- **Single binary** -- no dependencies on ca65, Python, or any external tools

## Documentation

- **[Language Guide](docs/language-guide.md)** -- complete reference for every language feature
- **[Architecture](docs/architecture.md)** -- compiler internals and module overview
- **[NES Reference](docs/nes-reference.md)** -- hardware quick reference for contributors
- **[Examples README](examples/README.md)** -- how to build and run examples

## Examples

| Example | Features demonstrated |
|---------|---------------------|
| [`hello_sprite.ne`](examples/hello_sprite.ne) | D-pad input, sprite drawing |
| [`bouncing_ball.ne`](examples/bouncing_ball.ne) | Automatic movement, edge detection |
| [`coin_cavern.ne`](examples/coin_cavern.ne) | Multi-state game, functions, constants, gravity |
| [`arrays_and_functions.ne`](examples/arrays_and_functions.ne) | Arrays, functions, while loops, inline functions |
| [`state_machine.ne`](examples/state_machine.ne) | State transitions, on enter/exit, timers |
| [`sprites_and_palettes.ne`](examples/sprites_and_palettes.ne) | Inline CHR data, palettes, scroll, type casting |
| [`mmc1_banked.ne`](examples/mmc1_banked.ne) | MMC1 mapper, bank declarations, multiply |
| [`structs_enums_for.ne`](examples/structs_enums_for.ne) | Structs, enums, `for` loops, struct literals |
| [`inline_asm_demo.ne`](examples/inline_asm_demo.ne) | Inline asm with `{var}` substitution, `poke`/`peek` |

## Compiler Commands

```bash
# Compile to ROM
nescript build game.ne

# Compile with custom output path
nescript build game.ne --output my_game.nes

# Type-check only (no ROM output)
nescript check game.ne

# View generated 6502 assembly
nescript build game.ne --asm-dump

# Enable debug mode
nescript build game.ne --debug
```

## Emulator Compatibility

Output ROMs are standard iNES format and work with any NES emulator:

- **[Mesen](https://www.mesen.ca/)** -- recommended, best debugging support
- **[FCEUX](https://fceux.com/)**
- **[Nestopia](http://nestopia.sourceforge.net/)**

## Project Status

NEScript implements all five planned milestones:

| Milestone | Status | Key Features |
|-----------|--------|-------------|
| M1: Hello Sprite | Done | Full compiler pipeline, assembler, ROM builder |
| M2: Game Loop | Done | Functions, arrays, IR, optimizer, call graph analysis |
| M3: Asset Pipeline | Done | PNG-to-CHR, sprites, palettes, debug symbols |
| M4: Optimization | Done | Strength reduction, ZP promotion, type casting, asm-dump |
| M5: Bank Switching | Done | MMC1/UxROM/MMC3, bank declarations, software mul/div |

210 tests across 14 modules, with CI running fmt, clippy, test, and example compilation on every push.

## License

[MIT](LICENSE)
