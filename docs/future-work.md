# Future Work

This document catalogs known gaps, incomplete features, and planned improvements
in the NEScript compiler. Items are organized by priority and area.

---

## 1. IR-Based Code Generation

**Status**: The IR pipeline (lowering + optimization) runs during compilation but
its output is discarded. Code generation still works from the AST directly
(`src/codegen/mod.rs`). This means IR-level optimizations have no effect on the
final ROM.

**What exists**:
- `src/ir/mod.rs`: Complete IR type definitions (`IrProgram`, `IrFunction`,
  `IrBasicBlock`, `IrOp`, `IrTerminator`)
- `src/ir/lowering.rs`: AST → IR translation for all statement and expression types
- `src/optimizer/mod.rs`: Constant folding, dead code elimination, strength
  reduction, ZP promotion analysis, function inlining — all operating on IR

**What's needed**:
- A new `src/codegen/ir_codegen.rs` that walks `IrProgram` and emits 6502
  `Instruction` sequences from `IrOp`/`IrTerminator` instead of from AST nodes
- Register allocation strategy for IR temps → A/X/Y/zero-page spill slots
- Replace the `CodeGen::generate(&program)` call in `main.rs` with
  `ir_codegen::generate(&ir_program, &analysis)`
- Once working, delete the AST-based codegen entirely

**IR lowering issues to fix first** (found during code review):
- `ButtonRead` emits `ReadInput` with no destination temp, then uses uninitialized
  temp in the `And` mask operation (`src/ir/lowering.rs:534`). The fix: `ReadInput`
  should store the input byte into a temp, or the lowering should emit
  `LoadVar(t, input_var_id)` after `ReadInput`.
- Logical AND/OR use raw `VarId(self.next_var_id)` for temp storage without
  registering it (`src/ir/lowering.rs:603,637`). Should use `IrTemp` instead.
- Break/continue create unreachable blocks that contain subsequent dead statements
  (`src/ir/lowering.rs:259`). Should either skip lowering after a terminator, or
  the dead code elimination pass should handle it.

**Impact**: Enables all optimizer passes to actually affect output quality.
Currently the optimizer is validated by tests but its results are thrown away.

---

## 2. Codegen Gaps (AST-Based)

These features are parsed and analyzed but produce no 6502 output:

| Feature | Location | Status |
|---------|----------|--------|
| Function calls | `codegen:148` | `Statement::Call` is a no-op |
| Return values | `codegen:148` | `Statement::Return` is a no-op |
| State transitions | `codegen:148` | `Statement::Transition` is a no-op |
| Array indexing | `codegen:199-200` | `LValue::ArrayIndex` assignment is a no-op |
| Array expressions | `codegen:417` | `Expr::ArrayIndex`, `Expr::ArrayLiteral` are no-ops |
| Function call expressions | `codegen:417` | `Expr::Call` returns nothing |
| Scroll | `codegen:151-152` | `Statement::Scroll` is a no-op |
| Load background | `codegen:154-155` | `Statement::LoadBackground` is a no-op |
| Set palette | `codegen:154-155` | `Statement::SetPalette` is a no-op |
| Multiply/divide/modulo | `codegen:450-452` | `BinOp::Mul/Div/Mod` only emit left operand |
| Dynamic shifts | `ir/lowering:575` | Shift amount is hardcoded to 1 |

**Priority fixes for a working multi-state game**:
1. **Function calls**: JSR to function label, pass args via zero-page, return via A
2. **State transitions**: Write state ID to a zero-page variable, jump to dispatcher
3. **Array indexing**: Use X register for index, LDA absolute,X for loads

---

## 3. Sprite Name Resolution

**Status**: `draw SpriteName at: (x, y)` parses the sprite name but codegen
ignores it. All draws use OAM slot 0 with CHR tile index 0 (the built-in smiley).

**What's needed**:
- Track a mapping from sprite name → CHR tile index in the linker
- When a `sprite` declaration provides inline CHR data or `@chr("file.png")`,
  write that data to the CHR ROM at a known tile index
- In codegen, look up the sprite name to get the tile index and write it to
  OAM byte 1 (tile number)
- Support multiple OAM slots: track a `next_oam_slot` counter per frame,
  allocate slots 0..63 as draws are emitted, warn if >64

---

## 4. Multi-OAM Sprite Support

**Status**: Every `draw` writes to the same OAM bytes ($0200-$0203). Only one
sprite is visible at a time.

**What's needed**:
- Frame-level OAM slot allocator: each `draw` gets the next available 4-byte slot
- Counter reset at the start of each frame handler
- Warn at compile time if a frame handler has >64 static draws
- Runtime: clear unused OAM slots to Y=$FE (off-screen) at frame start

---

## 5. State Machine Dispatch

**Status**: The codegen only generates code for the `start` state. Other states
are parsed and analyzed but their frame handlers are never called.

**What's needed**:
- A `current_state` zero-page variable holding the active state index
- A dispatch table at the start of the main loop: load `current_state`, branch to
  the corresponding state's frame handler
- `transition StateName` writes the new state index to `current_state`
- Generate `on_enter` call on transition, `on_exit` call before leaving

---

## 6. Include Directive

**Status**: The `include` keyword is lexed (`KwInclude`) but not parsed or
implemented.

**What's needed**:
- Parser: `include "path.ne"` at top level
- File resolution: relative to the including file's directory
- Circular include detection (track include stack)
- Merge included declarations into the main `Program` AST
- Span tracking: included files need their own `file_id` for error messages

---

## 7. Debug Mode

**Status**: `--debug` flag is accepted by the CLI but has no effect.

**What's needed**:
- `debug.log(expr, ...)`: write values to emulator debug port ($4800)
- `debug.assert(expr)`: emit runtime check, halt on failure
- `debug.overlay(x, y, text)`: render text to a reserved nametable region
- Array bounds checking in debug mode (compare index against array size)
- Frame overrun detection: count cycles per frame, warn if approaching vblank
- All debug code stripped in release mode (already designed: `DebugLog`/`DebugAssert`
  in the `Statement` enum are defined in the spec but not yet in the AST)

---

## 8. Scroll Hardware Writes

**Status**: `scroll(x, y)` is parsed but produces no output.

**What's needed**:
- Write X scroll value to PPU register $2005
- Write Y scroll value to PPU register $2005 (second write)
- Must happen during vblank (inside NMI handler or after `wait_frame`)
- Split-screen scroll requires MMC3 scanline IRQ (`on scanline`)

---

## 9. Asset Pipeline Completion

**Status**: PNG → CHR conversion exists (`src/assets/chr.rs`) but is never called
from the compilation pipeline.

### 9a. Wire `@chr("file.png")` to actual PNG loading
- When a sprite/background declares `chr: @chr("path.png")`, call `png_to_chr()`
  during compilation
- Resolve the path relative to the source file
- Store resulting CHR data in the ROM's CHR section at a known tile index

### 9b. Wire `@binary("file.bin")` to raw file inclusion
- Read the file as raw bytes and include in CHR or PRG ROM

### 9c. Palette extraction from PNG
- `@palette("file.png")`: analyze image colors, map to nearest NES palette entries
- Already have `nearest_nes_color()` in `src/assets/palette.rs`

### 9d. Nametable conversion
- Full 256×240 PNG → 960-byte nametable + 64-byte attribute table
- Tile deduplication (max 256 unique tiles per pattern table)
- Error if >256 unique tiles

---

## 10. Error Message Polish

**Status**: Errors work and render with ariadne, but many error paths use generic
messages.

### Unused error codes
These are defined in `ErrorCode` but never emitted:
- `E0202` — invalid cast
- `E0203` — invalid operation for type
- `E0301` — zero-page overflow
- `E0403` — unreachable state
- `E0505` — multiple start declarations
- `W0101` — expensive multiply/divide operation
- `W0102` — loop without break or wait_frame
- `W0103` — unused variable
- `W0104` — unreachable code after return/break/transition

### Missing validations
- No error for assigning to a `const`
- No error for `break`/`continue` outside a loop
- No warning for variables declared but never read
- No error for `return` with wrong type vs function signature
- No error for calling a function with wrong argument count/types

---

## 11. Scanline IRQ (MMC3)

**Status**: `on scanline(N)` is in the spec and `on_scanline` field exists in
`StateDecl`, but parsing and codegen are not implemented.

**What's needed**:
- Parser: `on scanline(N) { ... }` event handler in state bodies
- MMC3 IRQ setup: write scanline counter to $C000/$C001/$E000/$E001
- IRQ handler generation: branch to the scanline handler code
- Only valid with `mapper: MMC3`

---

## 12. Audio

**Status**: `play`, `start_music`, `stop_music` keywords are lexed but produce
no output. No audio driver exists.

**What's needed**:
- `@sfx("file.nsf")` / `@music("file.ftm")` asset directives
- Audio driver running in the NMI handler (after OAM DMA)
- `play SfxName` → trigger one-shot sound effect
- `start_music TrackName` / `stop_music` → start/stop background music
- FamiTracker export format parsing (complex — consider using existing tools)

---

## 13. Language Features (Post-v0.1)

From the spec's "Reserved for Future Versions" section:

| Feature | Description |
|---------|-------------|
| **Structs** | `struct Vec2 { x: u8, y: u8 }` — composite types with known layout |
| **Enums** | `enum Direction { Up, Down, Left, Right }` — mapped to u8 values |
| **Fixed-point** | `fixed8.8` type for sub-pixel movement |
| **Text/HUD** | Font sheet declarations, layout system for scores/health/menus |
| **Metasprites** | Multi-tile sprite groups with relative positioning |
| **Tilemaps** | Declarative level data with collision queries |
| **SRAM/saves** | Persistent storage declarations for battery-backed save data |
| **NES 2.0** | Extended iNES header format |

---

## 14. Inline Assembly

**Status**: `asm { }` blocks are lexed (`KwAsm`, `AsmBody`) but not parsed or
compiled. The lexer has raw-capture mode for asm content.

**What's needed**:
- Parser: capture asm body text, parse `{variable_name}` substitutions
- Codegen: emit raw 6502 instructions with variable address substitution
- Labels: local to the asm block scope
- `raw asm { }` variant with no substitution or safety checks

---

## 15. Compiler Performance

**Status**: Compilation is fast (<100ms for all examples) but has no benchmarks.

**What's needed**:
- `cargo bench` benchmarks for each pipeline phase
- Regression test: compilation must stay under 500ms for any reasonable project
- Profile-guided optimization of hot paths (lexer, parser)

---

## 16. WASM Build Target

**What's needed**:
- Factor out all file I/O behind a trait (`FileSystem` / `VFS`)
- Core pipeline takes `&str` source → `Vec<u8>` ROM with no filesystem access
- Compile the compiler to WASM for a browser-based IDE
- In-browser NES emulator integration for instant preview

---

## 17. Open Design Questions

From the engineering plan:

1. **Inline asm label syntax**: `.label:` (ca65 style) vs `label:` (generic)?
2. **Debug port address**: $4800 is conventional but not universal. Support
   multiple debug output methods?
3. **OAM allocation strategy**: Sequential allocation vs priority-based with
   automatic sprite cycling for the 8-per-scanline limit?
4. **Error recovery granularity**: How aggressively should the parser recover?
   More recovery = more errors per compile, but risk of cascading false errors.

---

## 18. Missing Assignment Operators

`<<=` and `>>=` (shift-assign) are lexed as `ShiftLeftAssign`/`ShiftRightAssign`
tokens but have no corresponding variants in the `AssignOp` AST enum. They can
never appear in parsed code. Adding them requires:
- New `AssignOp::ShiftLeftAssign` and `AssignOp::ShiftRightAssign` variants
- Parser handling in `parse_assign_or_call` (alongside the other compound ops)
- Codegen: load value, shift, store back

---

## 19. Player 2 Controller

`Player::P1` and `Player::P2` are defined in the AST but marked `#[allow(dead_code)]`.
The parser always produces `ButtonRead(None, ...)` — it never parses `p1.button.X`
or `p2.button.X` syntax. The runtime only reads controller 1 ($4016).

**What's needed**:
- Parser: `p1.button.X` and `p2.button.X` syntax producing `Player::P1`/`P2`
- Runtime: read controller 2 from $4017 into a second zero-page byte
- Codegen: select the correct input byte based on player

---

## 20. Register Allocator

The plan describes `src/codegen/regalloc.rs` for managing A/X/Y allocation, but
no register allocator exists. The current codegen uses A for everything and
spills to zero-page $02 for comparisons. A proper allocator would:
- Track A/X/Y liveness across basic blocks
- Use X for array indexing and loop counters
- Use Y as secondary index
- Spill to zero-page temps only when all three are live

---

## Priority Order

### Recently completed (removed from backlog)

These items were documented as future work but have since been implemented:

- **State machine dispatch** — CMP + BNE + JMP trampoline dispatch table,
  `current_state` in ZP $03, on_enter/on_exit handlers as JSR targets
- **Function call codegen** — JSR to function labels, ZP $04-$07 param passing,
  RTS for returns
- **Break/continue** — loop_stack with JMP to continue/break labels
- **Return** — evaluate expr to A + RTS
- **Transition** — write state index + JMP to main loop
- **Array indexing** — TAX + LDA/STA with ZeroPageX or AbsoluteX
- **Scroll** — PPU $2005 writes (X then Y)
- **Multiply/divide/modulo** — JSR __multiply/__divide with shift-and-add/restoring division
- **Shift left/right** — ASL A / LSR A
- **Multi-OAM sprites** — sequential slot allocation (0-63), reset per frame
- **Const assignment error** — E0203 for assigning to constants
- **Break outside loop error** — E0203 for break/continue without enclosing loop
- **Math routines wired into linker** — gen_multiply/gen_divide included in ROM
- **Sprite name resolution** — sprite declarations map to CHR tile indices,
  draw statements use the correct tile number
- **Inline sprite CHR data** — sprite decls with `chr: [0x..., ...]` work
- **Include directive** — `include "path"` inlines files at parse time,
  with circular include detection
- **Shift-assign operators** — `<<=` and `>>=` work in all contexts
- **Player 2 controller** — `p1.button.X` / `p2.button.X` syntax, P2 input
  read from $4017 into ZP $08
- **Unused variable warning** — W0103 emits for declared-but-never-read
  globals (underscore-prefix silences)
- **Unreachable state warning** — W0104 emits for states not reachable from
  the start state via transitions
- **E0502 "did you mean" suggestions** — undefined variable errors include
  a suggestion for nearby-named symbols
- **debug.log / debug.assert** — parses into `Statement::DebugLog` /
  `Statement::DebugAssert`, codegen emits runtime writes to $4800 when
  `--debug` is set, stripped in release mode
- **--debug CLI flag wired** — threads through `CodeGen::with_debug`
- **IR-based codegen** — `src/codegen/ir_codegen.rs` walks `IrProgram` and
  emits 6502 for every IR op: load/store, arithmetic, comparisons, arrays,
  calls, draws, input (P1 and P2), scroll, debug.log/assert, state
  dispatch, multi-OAM slot allocation, transitions + on_enter handlers.
  Now the default; `--use-ast` falls back to the legacy AST-based codegen.
- **IR lowering bug fixes** — `ReadInput` now has a destination temp,
  `ButtonRead` uses the proper input temp, logical AND/OR use a new
  `emit_move` helper instead of the buggy raw VarId temp storage
- **IR Player 2 controller** — `ReadInput(temp, player)` selects $01
  or $08 based on player index
- **IR scroll support** — `scroll(x, y)` lowers to `IrOp::Scroll(x, y)`
  which emits two PPU $2005 writes in IR codegen
- **IR debug.log / debug.assert** — new `IrOp::DebugLog(temps)` and
  `IrOp::DebugAssert(cond)` variants, emitted as $4800 writes in debug
  mode and stripped in release (same behavior as AST codegen)
- **Asset pipeline @binary / @chr loading** — `resolve_sprites()` reads
  raw binary files and converts PNGs via `png_to_chr()`. Missing files
  are silently skipped (documentation-friendly)
- **Call arity validation** — E0203 when `Statement::Call` or
  `Expr::Call` has the wrong number of arguments or a mismatched
  argument type (uses a `function_signatures` map)
- **Return type validation** — `return value` is type-checked against
  the function's declared return type (E0201); returning a value from a
  void function emits E0203
- **W0102 loop-without-yield warning** — emitted when a `loop { ... }`
  body contains no `break`, `return`, `transition`, or `wait_frame`
- **W0101 expensive mul/div/mod warning** — flags multiply/divide/modulo
  with two non-constant operands; literal operands are silent because
  the optimizer strength-reduces them
- **W0104 dead-code-after-terminator warning** — statements after
  `return`, `break`, `continue`, or `transition` in the same block
  emit W0104 with a label pointing at the terminator
- **E0301 RAM overflow** — the zero-page user region is now bounded
  above by `$80` (leaving `$80-$FF` for IR temps) and the main RAM
  allocator stops at `$0800`; overflow emits E0301 at the declaration
- **E0505 multiple start declarations** — parser rejects a second
  `start X` token
- **`on scanline(N)` parsing** — `state { on scanline(240) { ... } }`
  parses and populates `StateDecl::on_scanline`; analyzer emits E0203
  if the game isn't using MMC3. Codegen (MMC3 IRQ vector wiring) is
  still TODO
- **Inline assembly** — `asm { ... }` blocks. The lexer captures the
  body as a raw `AsmBody` token; `src/asm/inline_parser.rs` provides a
  minimal 6502 mnemonic parser that handles every addressing mode the
  codegen emits. Both IR and AST codegen splice parsed instructions
  directly into the output stream

### Remaining priority order

For someone picking up this codebase, the recommended order of work:

1. **Delete AST codegen** — IR codegen is now the default and matches
   all AST codegen features. Once confidence is high (e.g. a few weeks
   of game-writing), remove `--use-ast` and `src/codegen/mod.rs`'s
   AST-specific code. Keep the shared constants (`DEBUG_PORT`, ZP
   layout) in a common module.
2. **`on scanline` codegen** — parser and analyzer support are in
   place, but the MMC3 IRQ vector is still stubbed. Need to install an
   IRQ handler that dispatches to the right scanline block based on
   the counter latched in `$C000`/`$C001`.
3. **Audio** — SFX/music driver
4. **Language features** — structs, enums, fixed-point
5. **Register allocator** — proper A/X/Y allocation to replace
   zero-page spills used by the current IR codegen
