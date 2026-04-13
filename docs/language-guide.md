# NEScript Language Guide

NEScript is a statically-typed, compiled language designed for NES game development. It compiles directly to 6502 machine code packaged as iNES-format ROMs -- no external assembler or tooling required.

This guide covers every language feature with practical examples.

---

## Program Structure

Every NEScript program consists of a game declaration, top-level definitions, and a start declaration.

```
game "My Game" {
    mapper: NROM
    mirroring: vertical
}

const SPEED: u8 = 2

var score: u8 = 0

fun helper() -> u8 {
    return 42
}

state Title {
    on frame {
        draw Logo at: (100, 100)
        if button.start {
            transition Playing
        }
    }
}

state Playing {
    on enter {
        score = 0
    }
    on frame {
        // game logic here
    }
}

start Title
```

### Game Declaration

The `game` block is required and must appear first. It names the game and sets hardware configuration.

```
game "Coin Cavern" {
    mapper: NROM
    mirroring: vertical
}
```

Available properties:

| Property     | Values                           | Default      |
|--------------|----------------------------------|--------------|
| `mapper`     | `NROM`, `MMC1`, `UxROM`, `MMC3`  | required     |
| `mirroring`  | `horizontal`, `vertical`         | `horizontal` |

### Start Declaration

Exactly one `start` declaration must exist. It names the initial state entered on power-on.

```
start Title
```

---

## Types

NEScript has four primitive types and fixed-size arrays.

### Primitive Types

| Type   | Size    | Range           | Description                        |
|--------|---------|-----------------|------------------------------------|
| `u8`   | 1 byte  | 0 to 255        | Unsigned 8-bit integer             |
| `i8`   | 1 byte  | -128 to 127     | Signed 8-bit integer               |
| `u16`  | 2 bytes | 0 to 65535      | Unsigned 16-bit integer            |
| `bool` | 1 byte  | `true` / `false`| Boolean                            |

### Arrays

Arrays are fixed-size, homogeneous, and zero-indexed. The size must be a compile-time constant. Maximum 256 elements.

```
var enemies: u8[8]
const TABLE: u8[4] = [10, 20, 30, 40]
```

### Type Casting

NEScript has no implicit coercion. All conversions use `as`:

```
var a: u8 = 200
var b: u16 = a as u16       // zero-extend: 200
var c: i8 = a as i8         // reinterpret bits
var d: u8 = b as u8         // truncate to low byte
```

---

## Variables

### Variable Declarations

Variables are declared with `var` and must have an explicit type:

```
var x: u8                   // uninitialized (zeroed on state entry)
var y: u8 = 100             // initialized
var pos: u16 = 0x0400       // 16-bit value
var alive: bool = true
var scores: u8[4] = [0, 0, 0, 0]
```

### Constants

Constants are evaluated at compile time and stored in ROM:

```
const MAX_ENEMIES: u8 = 5
const SPEED: u8 = 3
const SIN_TABLE: u8[8] = [0, 49, 90, 117, 127, 117, 90, 49]
```

### Enums

Enums declare a named set of `u8` constants. Each variant is assigned an
index starting at 0 in declaration order:

```
enum Direction { Up, Down, Left, Right }
// Up=0, Down=1, Left=2, Right=3

var player_dir: u8 = Up

on frame {
    if button.left  { player_dir = Left }
    if button.right { player_dir = Right }
    if player_dir == Down { /* ... */ }
}
```

Variant names are global — they are flattened into the top-level symbol
table, so a variant cannot share its name with any other constant,
variable, or function (E0501). An enum cannot have more than 256
variants because each is stored as a `u8`.

### Structs

Structs declare composite types with named fields:

```
struct Vec2 {
    x: u8,
    y: u8,
}

struct Player {
    health: u8,
    lives: u8,
}

var pos: Vec2
var hero: Player

on frame {
    pos.x = 100
    pos.y = 50
    hero.health = 3
    hero.lives = 5
    if button.right { pos.x += 1 }
    draw Hero at: (pos.x, pos.y)
}
```

Fields are laid out contiguously in declaration order. A variable of
struct type allocates enough contiguous bytes to hold all its fields;
each field is accessible via the dot operator.

Struct literals initialize or assign all fields at once:

```
struct Vec2 { x: u8, y: u8 }

// as an initializer
var pos: Vec2 = Vec2 { x: 100, y: 50 }

// as an assignment
on frame {
    pos = Vec2 { x: 0, y: 0 }
    if button.right {
        pos = Vec2 { x: pos.x + 1, y: pos.y }
    }
}
```

Inside `if`, `while`, and `for` conditions the struct literal syntax
is reserved for the following block, so wrap the literal in parens if
you ever need one in a condition:

```
if pos == (Vec2 { x: 0, y: 0 }) { /* ... */ }
```

In v0.1 only primitive field types (`u8`, `i8`, `bool`) are supported —
nested structs, `u16`, and array fields are not yet allowed.

### Memory Placement Hints

The NES has 256 bytes of zero-page RAM with faster access. You can hint where variables should be placed:

```
fast var px: u8             // prefer zero-page (faster instructions)
slow var high_score: u16    // prefer upper RAM (saves zero-page space)
var normal: u8              // compiler decides automatically
```

If zero-page is exhausted and `fast` variables cannot be placed, the compiler emits error `E0301`.

### Scope

| Scope    | Declared In    | Lifetime                                   |
|----------|----------------|---------------------------------------------|
| Global   | Top level      | Entire program, permanent RAM allocation    |
| State    | `state` block  | Active while state is active; RAM reusable  |
| Function | `fun` block    | Duration of function call                   |
| Block    | `if`/`while`   | Enclosing block, shares parent allocation   |

---

## Functions

### Declaration

Functions use `fun`, with optional parameters and return type:

```
fun add(a: u8, b: u8) -> u8 {
    return a + b
}

fun reset_score() {
    score = 0
}
```

### Inline Functions

The `inline` keyword hints the compiler to inline the function at call sites:

```
inline fun clamp(val: u8, max: u8) -> u8 {
    if val > max {
        return max
    }
    return val
}
```

`inline` is a hint -- the compiler may decline for large functions.

### Calling Functions

```
var result: u8 = add(10, 20)
reset_score()
```

### Restrictions

- **No recursion.** Both direct and indirect recursion are compile errors (`E0402`).
- **Call depth limit.** The default maximum call depth is 8. Exceeding it produces error `E0401`.

---

## States

States are the top-level organizational unit. Exactly one state is active at any time.

### State Declaration

```
state Playing {
    var timer: u8 = 0           // state-local variable

    on enter {
        // runs once when entering this state
        timer = 60
    }

    on exit {
        // runs once when leaving this state
    }

    on frame {
        // runs every frame (60 Hz) while this state is active
        timer -= 1
        draw Player at: (player_x, player_y)
    }
}
```

`on frame` is syntactic sugar for a loop with an implicit `wait_frame()` at the end. A state can have any combination of `on enter`, `on exit`, and `on frame`.

### State Transitions

```
transition GameOver
```

Transitions are immediate. The current state's `on exit` runs, then the target state's `on enter` runs. The remainder of the current frame handler does not execute.

---

## Expressions

### Literals

```
42              // decimal integer
0xFF            // hexadecimal
0b10110001      // binary
1_000           // underscores allowed for readability (if supported)
true            // boolean
false           // boolean
[1, 2, 3]      // array literal
```

All integer literals must fit in `u16` (0-65535). The compiler narrows to the required type at usage.

### Arithmetic Operators

| Operator | Description    | Example      |
|----------|----------------|--------------|
| `+`      | Addition       | `a + b`      |
| `-`      | Subtraction    | `a - b`      |
| `*`      | Multiplication | `a * b`      |
| `/`      | Division       | `a / b`      |
| `%`      | Modulo         | `a % b`      |

`*`, `/`, and `%` are available but expensive on the 6502 (software routines). The compiler optimizes power-of-two operations to shifts and warns on non-power-of-two multiply/divide.

### Bitwise Operators

| Operator | Description    | Example      |
|----------|----------------|--------------|
| `&`      | Bitwise AND    | `a & 0x0F`   |
| `\|`     | Bitwise OR     | `a \| 0x80`  |
| `^`      | Bitwise XOR    | `a ^ mask`   |
| `~`      | Bitwise NOT    | `~a`         |
| `<<`     | Shift left     | `a << 2`     |
| `>>`     | Shift right    | `a >> 1`     |

### Comparison Operators

| Operator | Description       | Example      |
|----------|-------------------|--------------|
| `==`     | Equal             | `a == 0`     |
| `!=`     | Not equal         | `a != b`     |
| `<`      | Less than         | `a < 10`     |
| `>`      | Greater than      | `a > max`    |
| `<=`     | Less or equal     | `a <= 255`   |
| `>=`     | Greater or equal  | `a >= min`   |

### Logical Operators

NEScript uses keyword-based logical operators:

```
if alive and (health > 0) {
    // ...
}
if not paused or force_update {
    // ...
}
```

| Operator | Description   |
|----------|---------------|
| `and`    | Logical AND   |
| `or`     | Logical OR    |
| `not`    | Logical NOT   |

### Operator Precedence

From highest to lowest:

| Level | Operators                          | Associativity |
|-------|------------------------------------|---------------|
| 1     | `()` grouping                      | --            |
| 2     | `-` (unary), `~`, `not`            | right         |
| 3     | `*`, `/`, `%`                      | left          |
| 4     | `+`, `-`                           | left          |
| 5     | `<<`, `>>`                         | left          |
| 6     | `&`                                | left          |
| 7     | `^`                                | left          |
| 8     | `\|`                               | left          |
| 9     | `==`, `!=`, `<`, `>`, `<=`, `>=`   | left          |
| 10    | `and`                              | left          |
| 11    | `or`                               | left          |

### Button Reads

Read controller input as boolean expressions:

```
if button.right {
    player_x += SPEED
}
if button.a {
    jump()
}
```

Available buttons: `up`, `down`, `left`, `right`, `a`, `b`, `start`, `select`.

For two-player games, prefix with the player:

```
if p1.button.a { /* player 1 */ }
if p2.button.right { /* player 2 */ }
```

Without a prefix, `button` refers to player 1.

### Function Calls in Expressions

```
var clamped: u8 = clamp_x(player_x + SPEED)
```

### Array Indexing

```
var val: u8 = table[i]
table[i] = 0
```

### Type Casting

```
var wide: u16 = narrow as u16
```

---

## Statements

### Assignment

```
x = 10
x += 5
x -= 1
x &= 0x0F
x |= 0x80
x ^= mask
```

All assignment operators:

| Operator | Description         |
|----------|---------------------|
| `=`      | Assign              |
| `+=`     | Add and assign      |
| `-=`     | Subtract and assign |
| `&=`     | AND and assign      |
| `\|=`    | OR and assign       |
| `^=`     | XOR and assign      |

Array element assignment:

```
enemies[i] = 0
scores[player] += 10
```

### If / Else If / Else

Braces are always required. No ternary operator.

```
if health == 0 {
    transition GameOver
} else if health < 3 {
    flash_warning()
} else {
    // normal gameplay
}
```

### While Loop

```
var i: u8 = 0
while i < 10 {
    enemies[i] = 0
    i += 1
}
```

### Match Statement

`match` matches a scrutinee against a sequence of patterns and
executes the body of the first matching arm. Each arm's pattern is
compared against the scrutinee with `==`. An underscore arm `_` acts
as the catch-all:

```
enum State { Title, Playing, GameOver }
var state: u8 = Title

on frame {
    match state {
        Title => {
            if button.start { state = Playing }
        }
        Playing => {
            // ... game logic ...
        }
        GameOver => {
            if button.a { state = Title }
        }
        _ => {}
    }
}
```

`match` desugars to an `if` / `else if` chain at parse time, so
patterns can be any expression that produces a value comparable to
the scrutinee.

### For Loop

The `for` loop iterates over a half-open integer range `[start, end)`:

```
for i in 0..8 {
    total += arr[i]
}
```

The loop variable is a `u8` scoped to the loop body. Both bounds can
be any expression that evaluates to `u8` at runtime, including
constants or variables. The range is half-open, so `0..8` iterates
`0, 1, 2, ..., 7` (8 iterations). For a closed range, use `0..9`.

The loop is desugared into a `while` loop with an index variable, so
`break` and `continue` work the same as in any loop body.

### Loop (Infinite)

```
loop {
    wait_frame()
    if button.start {
        break
    }
}
```

The compiler warns if a `loop` contains neither `break`, `wait_frame`, nor `transition`.

### Break and Continue

```
var i: u8 = 0
while i < 20 {
    i += 1
    if enemies[i] == 0 {
        continue            // skip inactive enemies
    }
    if i > 10 {
        break               // stop processing
    }
    update_enemy(i)
}
```

### Return

```
fun abs_diff(a: u8, b: u8) -> u8 {
    if a > b {
        return a - b
    }
    return b - a
}
```

Functions without a return type use `return` with no value (or simply reach the end of the function body).

### Draw

Render a sprite to the screen:

```
draw Player at: (player_x, player_y)
draw Coin at: (COIN_X, COIN_Y) frame: anim_frame
```

The `draw` statement writes to the OAM shadow buffer. The NES supports up to 64 sprites per frame.

Syntax: `draw SpriteName at: (x_expr, y_expr) [frame: expr]`

### Transition

Switch to another state immediately:

```
transition GameOver
```

The current state's `on exit` runs, then the target state's `on enter` runs.

### Wait Frame

Yield execution until the next vertical blank (NMI). Synchronizes to the 60 Hz display refresh.

```
wait_frame()
```

This triggers OAM DMA transfer and PPU updates before yielding. Inside `on frame`, a `wait_frame()` is implicit at the end of each frame.

### Scroll

Set the PPU scroll position:

```
scroll(scroll_x, scroll_y)
```

### Function Calls as Statements

```
reset_score()
update_physics(player_x, player_y)
```

---

## Assets

### Sprite Declarations

```
sprite Player {
    chr: @chr("assets/player.png")
}

sprite Coin {
    chr: @binary("assets/coin.bin")
}
```

### Asset Sources

Three ways to provide asset data:

| Source                     | Description                           |
|----------------------------|---------------------------------------|
| `@chr("file.png")`        | Convert PNG to CHR tile data          |
| `@binary("file.bin")`     | Include raw binary data verbatim      |
| Inline `[0x00, 0x7E, ...]`| Hex byte array directly in source     |

---

## Audio

NEScript ships with a full data-driven audio subsystem. Sound effects run on pulse channel 1 and music runs on pulse channel 2, both driven by an NMI-time tick that walks per-track data tables compiled into PRG ROM. Programs that never touch audio pay zero ROM or cycle cost — the driver and its period table are only linked in when user code contains at least one `play`, `start_music`, or `stop_music` statement.

### Statements

```
play SfxName          // trigger a one-shot sound effect
start_music TrackName // begin looping background music
stop_music            // silence the music channel
```

Each statement looks up the name in the program's user declarations first, then falls back to the builtin table. Unknown names are a hard error (E0505).

### SFX Declarations

An `sfx` block is a frame-accurate envelope for pulse 1. `pitch` latches the pulse period on trigger; `volume` runs one entry per frame, so the envelope length controls the effect duration.

```
sfx Pickup {
    duty: 2                                   // 0-3, 2 = 50% square (default)
    pitch: [0x50, 0x50, 0x50, 0x50, 0x50]     // period for each frame
    volume: [15, 12, 9, 6, 3]                  // 0-15, one per frame
}
```

Rules:
- `pitch` and `volume` must have the same length (the frame count).
- `volume` values are 0-15 (4-bit pulse volume).
- `duty` is 0-3 and defaults to 2.
- Maximum 120 frames (2 seconds at 60 fps).

### Music Declarations

A `music` block is a flat list of `(pitch, duration)` note pairs played on pulse 2. Pitch 0 is a rest; pitches 1-60 are indices into the builtin 60-note period table (C1 through B5, with middle C at index 37). Duration is in frames (so at 60 fps, `30` is half a second).

```
music Theme {
    duty: 2                                   // 0-3 (default 2)
    volume: 10                                // 0-15 (default 10)
    repeat: true                              // loop when track ends (default true)
    notes: [
        37, 20,    // C4 for 20 frames
        41, 20,    // E4
        44, 20,    // G4
        49, 20,    // C5
        0, 10,     // rest for 10 frames
    ]
}
```

Rules:
- `notes` must contain an even number of bytes (pitch + duration pairs).
- Pitches are 0 (rest) or 1-60 (period table index).
- Duration must be ≥ 1 frame.
- Maximum 256 notes per track.

### Builtin Names

For programs that want classic game audio without writing data tables, NEScript provides a handful of builtin effects and tracks that can be used directly:

**Builtin SFX**

| Name | Description |
|------|-------------|
| `coin`, `pickup`, `collect` | Ascending high blip |
| `jump`, `hop` | Descending arc |
| `hit`, `damage`, `explode` | Low blast |
| `click`, `select`, `confirm` | Sharp beep |
| `cancel`, `back`, `error` | Low longer tone |
| `shoot`, `laser`, `fire` | Very high pulse |
| `step`, `footstep` | Short low thud |

**Builtin Music**

| Name | Description |
|------|-------------|
| `title`, `theme`, `main` | Major arpeggio (looping) |
| `battle`, `boss` | Driving pulse (looping) |
| `win`, `victory`, `fanfare` | Ascending burst (one-shot) |
| `gameover`, `lose`, `fail` | Descending dirge (looping) |

A user-declared `sfx` or `music` block takes priority over a builtin with the same name, so `sfx coin { ... }` will shadow the default coin effect.

### How It Works

Compile time:

1. The resolver compiles each `sfx` into `(period_lo, period_hi, envelope[])` and each `music` into `(header, (pitch, duration)[])`, appending builtins for any referenced name that isn't user-declared.
2. The IR codegen emits `play Name` as: write trigger bytes to `$4002`/`$4003`, load envelope pointer into `$0C/$0D`, set the sfx counter. `start_music Name` stamps a state byte into `$07`, loads the stream pointer into `$0E/$0F` (and the loop base into `$05/$06`), and primes the duration counter.
3. The linker splices the audio tick, the 60-entry period table, and every compiled sfx/music blob into PRG ROM, all guarded on a `__audio_used` marker label so silent programs never pay the cost.

Runtime (every NMI, if audio is in use):

1. **SFX**: if the counter is nonzero, read one envelope byte through `(ZP_SFX_PTR),Y` and write it to `$4000`. A zero sentinel mutes pulse 1 and stops the tick.
2. **Music**: if active and the note counter hits zero, read the next pitch byte. 0 = rest (mute pulse 2). 1-60 = look up the period in the table and write to `$4006`/`$4007`. `0xFF` = loop back to the base pointer (or mute if `repeat: false`). Then read the duration byte and reload the counter.

Total memory cost: 8 bytes of zero page, ~200 bytes for the driver body, 120 bytes for the period table, plus the data for each user-declared sfx/music.

---

## Mappers

The mapper determines cartridge hardware and available ROM size.

| Mapper  | PRG ROM       | CHR ROM        | Features                         |
|---------|---------------|----------------|----------------------------------|
| `NROM`  | 16 or 32 KB   | 8 KB           | No banking, simplest             |
| `MMC1`  | Up to 256 KB  | Up to 128 KB   | Switchable banks                 |
| `UxROM` | Up to 256 KB  | 8 KB CHR RAM   | PRG banking only                 |
| `MMC3`  | Up to 512 KB  | Up to 256 KB   | Scanline counter, banking        |

### Bank Declarations

For mappers with bank switching:

```
bank MainCode {
    // Always-resident code (NMI handler, core engine)
}

bank Level1 {
    state Level1 { ... }
    background Level1BG { ... }
}
```

Banks can hold `prg` (code/data) or `chr` (graphics) content. Transitions between states in different banks automatically emit bank-switch and trampoline code.

---

## Comments

```
// Line comment -- extends to end of line

/* Block comment
   spans multiple lines */
```

---

## Includes

Split your game across multiple files:

```
include "physics.ne"
include "enemies.ne"
```

Includes are resolved relative to the including file. Circular includes are a compile error. Duplicate includes are skipped automatically.

---

## Debug Mode

Compile with `--debug` to enable runtime instrumentation. All debug features are stripped completely in release builds (zero bytes, zero cycles).

### Debug Logging

```
debug.log("Player position: ", px, ", ", py)
```

### Debug Assertions

```
debug.assert(lives > 0, "Lives should never be negative")
```

### Runtime Checks (Debug Only)

In debug mode, the compiler inserts:
- Array bounds checking on indexed access
- Arithmetic overflow warnings
- Stack depth monitoring at function entry
- Frame overrun detection (warns if frame handler exceeds vblank period)

---

## Hardware Intrinsics

For the common case of reading or writing a single PPU/APU/mapper
register, NEScript provides two built-in intrinsics:

```
poke(0x2006, 0x3F)        // write $3F to PPU address register
poke(0x2006, 0x00)        // (second half of the address)
poke(0x2007, 0x0F)        // write a palette byte to PPU data

var status: u8 = peek(0x2002)  // read PPU status register
```

The address argument to both is a compile-time constant. Zero-page
addresses compile to `STA $XX` / `LDA $XX`; anything larger compiles
to absolute addressing.

## Inline Assembly

For more elaborate sequences, use `asm { ... }` blocks:

```
fun fast_shift(input: u8) -> u8 {
    var result: u8 = 0
    asm {
        LDA {input}
        ASL A
        ASL A
        STA {result}
    }
    return result
}
```

Inside an `asm` block, `{name}` is replaced with the resolved
zero-page or absolute address of the variable `name`. Labels
defined with `name:` are local to the block.

### Raw Assembly

```
raw asm {
    LDA #$42
    STA $2007
}
```

`raw asm` skips variable substitution — `{name}` is passed through
verbatim. Useful for completely unmanaged snippets that don't
reference NEScript variables.

---

## Error Codes

### Lexer Errors (E01xx)

| Code   | Description                |
|--------|----------------------------|
| E0101  | Unterminated string literal |
| E0102  | Invalid character           |
| E0103  | Number literal overflow     |

### Type Errors (E02xx)

| Code   | Description                |
|--------|----------------------------|
| E0201  | Type mismatch              |
| E0203  | Invalid operation for type |

### Memory Errors (E03xx)

| Code   | Description                |
|--------|----------------------------|
| E0301  | Zero-page overflow         |

### Control Flow Errors (E04xx)

| Code   | Description                |
|--------|----------------------------|
| E0401  | Call depth exceeded        |
| E0402  | Recursion detected         |
| E0404  | Transition to undefined state |

### Declaration Errors (E05xx)

| Code   | Description                |
|--------|----------------------------|
| E0501  | Duplicate declaration      |
| E0502  | Undefined variable         |
| E0503  | Undefined function         |
| E0504  | Missing start declaration  |
| E0505  | Multiple start declarations|

### Warnings (W01xx)

| Code   | Description                              |
|--------|------------------------------------------|
| W0101  | Expensive multiply/divide operation      |
| W0102  | Loop without break or wait_frame         |
| W0103  | Unused variable                          |
| W0104  | Unreachable code (after return/break/transition, or state unreachable from start) |

### Example Error Output

```
error[E0201]: type mismatch
  --> game.ne:42:15
   |
42 |   var x: u8 = -5
   |               ^^ expected u8, found negative integer
   |
   = help: use i8 if you need negative values: var x: i8 = -5
```

```
error[E0402]: recursion is not allowed
  --> game.ne:55:5
   |
55 |     flood_fill(x + 1, y)
   |     ^^^^^^^^^^^^^^^^^^^^
   |
   = note: flood_fill calls itself (directly recursive)
   = help: the NES has only 256 bytes of stack; use an iterative algorithm instead
```

---

## Command Line

Compile a `.ne` source file into a `.nes` ROM:

```
nescript build game.ne
nescript build game.ne --output my_game.nes
nescript build game.ne --debug
nescript build game.ne --asm-dump
nescript build game.ne --dump-ir
```

| Flag            | Description                                                    |
|-----------------|----------------------------------------------------------------|
| `--output`      | Set output ROM file path (default: input.nes)                  |
| `--debug`       | Enable debug mode with runtime checks                          |
| `--asm-dump`    | Dump generated 6502 assembly to stdout                         |
| `--dump-ir`     | Dump the lowered IR program (after optimization) to stdout     |
| `--memory-map`  | Dump a memory map of variable allocations to stdout            |
| `--call-graph`  | Dump a call graph (which handler/function calls which) to stdout |

### Check

Type-check a source file without producing a ROM:

```
nescript check game.ne
```

---

## Complete Example

A full game demonstrating states, input, functions, constants, and transitions:

```
game "Coin Cavern" {
    mapper: NROM
}

const SPEED: u8 = 2
const SCREEN_RIGHT: u8 = 240
const COIN_X: u8 = 180
const COIN_Y: u8 = 100

var player_x: u8 = 40
var player_y: u8 = 200
var score: u8 = 0
var coins_left: u8 = 3

fun clamp_x(val: u8) -> u8 {
    if val > SCREEN_RIGHT {
        return 0
    }
    return val
}

state Title {
    on frame {
        draw Logo at: (100, 100)
        if button.start {
            transition Playing
        }
    }
}

state Playing {
    on enter {
        player_x = 40
        player_y = 200
        score = 0
        coins_left = 3
    }

    on frame {
        if button.right {
            player_x += SPEED
            if player_x > SCREEN_RIGHT {
                player_x = SCREEN_RIGHT
            }
        }
        if button.left {
            if player_x >= SPEED {
                player_x -= SPEED
            } else {
                player_x = 0
            }
        }

        if player_x >= COIN_X {
            if player_y >= COIN_Y {
                score += 1
                coins_left -= 1
                if coins_left == 0 {
                    transition GameOver
                }
            }
        }

        draw Player at: (player_x, player_y)
        draw Coin at: (COIN_X, COIN_Y)
    }
}

state GameOver {
    on frame {
        draw Trophy at: (120, 100)
        if button.start {
            transition Title
        }
    }
}

start Title
```

Build and run:

```
nescript build coin_cavern.ne
# produces coin_cavern.nes -- open in any NES emulator
```
