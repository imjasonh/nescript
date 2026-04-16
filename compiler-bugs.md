// compiler-bugs.md — a running log of compiler issues surfaced
// while implementing the Pong example (examples/pong.ne et al).
//
// Format, one entry per bug:
//
// ## #N — one-line title
//
// **Status**: OPEN / WORKED-AROUND / FIXED
// **Phase**:   lexer / parser / analyzer / ir / optimizer / codegen / linker / runtime / asset
// **Surfaced in**: examples/pong/<file>.ne (brief context)
//
// ### Reproducer
// ```ne
// ... minimal .ne snippet that triggers the bad behaviour ...
// ```
//
// ### Expected vs actual
// What the user-visible behaviour should be; what the compiler actually does.
//
// ### Workaround (if applied)
// The current shape of the code in examples/pong/ that avoids the bug,
// and exactly what should be reverted once the fix lands. Every workaround
// in examples/pong/ MUST be tagged with `// BUG: compiler-bugs.md #N` so
// grep -r "BUG: compiler-bugs.md" finds every reverible workaround in one pass.
//
// ### Guess at the fix
// Which source file(s) and what kind of change is likely needed. Doesn't
// have to be right — it's a hint for the compiler-bug cleanup milestone.
//
// ---

(no bugs logged yet — pong development just started)

---

## #1 — inline `asm { {param} }` resolves to an address nothing writes to

**Status**: FIXED in `src/codegen/ir_codegen.rs::IrCodeGen::new`
(the codegen now reads each function-local's address out of the
analyzer's `VarAllocation` table instead of minting its own
parallel `$0300+` range). The workaround in
`examples/sha256/sha_core.ne` — reading parameters directly from
the `$04`/`$05` transport slots — has been reverted in the same
commit.
**Phase**: codegen (prologue spill vs. inline-asm resolver
disagree on local addresses)
**Surfaced in**: `examples/sha256/sha_core.ne` — the 20-odd
32-bit byte primitives (`cp_wk`, `xor_wk`, `add_wk`, `rotr1_wk`,
`add_wk_to_h`, `add_k_to_wk`, …) all pass `dst` / `src` /
`w_ofs` / `h_ofs` / `k_ofs` as parameters and want to use them
inside `LDX {dst}` / `LDY {src}` / `LDA {wk},X`.

### Reproducer

```ne
game "Param Bug" { mapper: NROM }

var sink: u8 = 0

fun echo(value: u8) {
    asm {
        LDA {value}
        STA {sink}
    }
}

on frame {
    echo(0x42)
    if sink == 0x42 {
        draw Smiley at: (120, 120)     // should draw — doesn't
    }
}

start Main
```

`sink` is `0x00` every frame no matter what `echo` is called with.
`{value}` resolves to a zero-page slot that nothing in the
generated program ever writes to.

### Expected vs actual

**Expected** — the `asm { LDA {value} }` inside `echo` should load
the caller's argument. `sink` should become `0x42` after
`echo(0x42)` runs.

**Actual** — the function prologue reads `$04` (the parameter
transport slot) and spills it to one absolute address; the inline
`{value}` substitution resolves `value` to a different zero-page
address; nothing ever writes the spilled value to that zero-page
slot, so `LDA {value}` always loads whatever the RAM clear left
there (`0x00`).

A minimal `--asm-dump` shows the disagreement directly. For a
`fun cp_wk(dst: u8, src: u8) { asm { LDX {dst}; ... } }`:

```
__ir_fn_cp_wk:
    LDA ZeroPage(4)
    STA Absolute(1464)       ; $05B8 — codegen's address for `dst`
    LDA ZeroPage(5)
    STA Absolute(1465)       ; $05B9 — codegen's address for `src`
__ir_blk_fn_cp_wk_entry_1:
    LDX ZeroPage(39)         ; $27   — analyzer's address for `dst`
    LDY ZeroPage(40)         ; $28   — analyzer's address for `src`
    LDA AbsoluteY(1360)      ; wk,Y
    STA AbsoluteX(1360)
    ...
```

`$05B8` / `$05B9` are the codegen's spill destinations for the
function's locals. `$27` / `$28` are the analyzer's allocations
for the same two parameter names. Nothing copies `$05B8` → `$27`,
so the `LDX ZeroPage(39)` above always reads `0`.

`--memory-map` confirms the analyzer thinks the parameters live
in zero page:

```
$0027    [USER]    __local__cp_wk__dst (u8)
$0028    [USER]    __local__cp_wk__src (u8)
```

while `--asm-dump` shows the codegen's prologue writing them to
`$05B8` / `$05B9`.

### Root cause

Two independently-populated address maps disagree on where every
function-local lives:

- `src/analyzer/mod.rs::register_const` (for const decls) and
  the equivalent path for function parameters call
  `allocate_ram(size, span)`, which allocates from zero page and
  pushes a `VarAllocation { name: "__local__cp_wk__dst", address:
  0x0027, size: 1 }` onto `self.var_allocations`. This is the
  table `substitute_asm_vars` consults to resolve `{name}` inside
  `asm { ... }` blocks.

- `src/codegen/ir_codegen.rs::Emitter::new` (around line 255)
  **overwrites** every local's address in its own `var_addrs`
  map:

  ```rust
  let mut local_ram_next: u16 = 0x0300;
  // ... (skip past globals) ...
  for func in &ir.functions {
      for local in &func.locals {
          var_addrs.insert(local.var_id, local_ram_next);
          var_sizes.insert(local.var_id, local.size);
          local_ram_next += local.size.max(1);
      }
  }
  ```

  `local_ram_next` grows linearly from `0x0300` upward, past every
  other local in every other function. NEScript code generated
  afterwards — assignments, reads, arithmetic, the function's
  parameter spill prologue at `gen_function` — all consult
  `var_addrs` and therefore use the `$05B8`-ish codegen address.

  The comment on that block explains that the override is
  deliberate (so nested calls don't trash the caller's params
  when they overwrite `$04-$07`), but it stops tracking the
  analyzer's allocation entirely, so anyone else who still uses
  the analyzer's allocations (= the inline-asm resolver) sees a
  stale address.

- `src/codegen/ir_codegen.rs::substitute_asm_vars` (line 1371):

  ```rust
  self.allocations
      .iter()
      .find(|a| a.name == qualified)
      .map(|a| a.address)
  ```

  `self.allocations` is the `&[VarAllocation]` from the analyzer.
  That's the stale table — it still says `dst` is at `$27`.

### Blast radius

Silently wrong for every `fun` (regular or state-handler helper)
that references a parameter or a function-local `var` inside an
inline `asm { ... }` block. Globals and state-scoped (non-
function) locals are unaffected because the analyzer and codegen
agree on their addresses through `allocations`. The bug hides
itself well because the asm reads a zero-page slot that's always
`0` (the RAM clear zeros it, and nothing else writes there) —
most programs just produce a wrong result rather than crashing.

`examples/inline_asm_demo.ne` is also affected but its output
looks plausibly animated anyway:

```ne
fun times_four(input: u8) -> u8 {
    var result: u8 = input
    asm {
        LDA {result}    ; reads stale $14 (= 0), not $0301
        ASL A
        ASL A
        STA {result}    ; writes 0 << 2 = 0 to $14
    }
    return result       ; returns the $0301 copy of `input`, unchanged
}
```

So `times_four(x)` actually returns `x`, not `x * 4`. The
committed golden for that example reflects the bug rather than
the intended `×4` behaviour.

### How it was fixed

Option (a) from the original writeup: `IrCodeGen::new` now looks
each function-local's address up in the analyzer's
`VarAllocation` table instead of minting a parallel `$0300+`
range. The codegen and the inline-asm resolver consequently
agree on every local's address, so `{dst}` / `{src}` / … inside
`asm { ... }` blocks resolve to the same slot the NEScript-level
code reads and writes.

```rust
// Was:
let mut local_ram_next: u16 = 0x0300;
// ...
for func in &ir.functions {
    for local in &func.locals {
        var_addrs.insert(local.var_id, local_ram_next);
        var_sizes.insert(local.var_id, local.size);
        local_ram_next += local.size.max(1);
    }
}

// Is now:
for func in &ir.functions {
    let scope = scope_prefix_for_fn(&func.name);
    for local in &func.locals {
        let qualified = format!("__local__{scope}__{}", local.name);
        if let Some(alloc) = allocations.iter().find(|a| a.name == qualified) {
            var_addrs.insert(local.var_id, alloc.address);
            var_sizes.insert(local.var_id, alloc.size);
        }
    }
}
```

The same commit:

- factors the "function name → analyzer scope prefix" mapping
  (`_frame` / `_enter` / `_exit` / `_scanline_N` / bare name)
  into a `scope_prefix_for_fn(&str) -> String` helper and
  reuses it in `gen_function` so the two sites can't drift;
- updates `gen_function_prologue_spills_params_to_local_ram`
  (the regression test originally guarding the War-era param
  clobbering bug) to assert the spill's destination is *any*
  address outside `$04-$07`, not specifically `$0300+`. The
  invariant that matters is "separate from the transport slots",
  which holds for the analyzer's zero-page allocations too;
- reverts the `LDX $04` / `LDY $05` workaround across every
  primitive in `examples/sha256/sha_core.ne` back to the
  intended `LDX {dst}` / `LDY {src}` substitution form, and
  drops the "Parameter convention" note from the top of the
  file;
- regenerates `tests/emulator/goldens/inline_asm_demo.png`:
  that example's `times_four` previously returned its input
  verbatim (the inline asm operated on an unrelated zero-page
  byte that was always `0`), so the golden's smiley position
  drifted by exactly the expected `x * 4 mod 256` delta at
  frame 180.

Verified after the fix:

- `cargo test --all-targets` — 616 + 3 + 75 tests pass on
  both rustc 1.94.1 and 1.95.0.
- `cargo clippy --all-targets -- -D warnings` clean on both.
- Full emulator harness — 34/34 ROMs match their goldens
  (only `inline_asm_demo.png` changed, and the new capture
  reflects the corrected `×4` behaviour).
- The SHA-256 example still computes `AE9145DB…4E0D` for the
  auto-demo input `"NES"`, matching `shasum` byte-for-byte,
  with the inline-asm-pretty `{dst}` / `{src}` primitives.

---
