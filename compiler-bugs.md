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

**Status**: WORKED-AROUND (every SHA-256 primitive in
`examples/sha256/sha_core.ne` reads parameters straight out of the
caller's `$04`/`$05` transport slots instead of using `{dst}` /
`{src}`)
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

### Workaround (applied in `examples/sha256/`)

Every primitive in `sha_core.ne` reads its parameters straight
out of the transport slots `$04` / `$05` with the raw literal:

```ne
fun cp_wk(dst: u8, src: u8) {
    asm {
        LDX $04          ; == dst on entry
        LDY $05          ; == src on entry
        LDA {wk},Y
        STA {wk},X
        ; ... 3 more 4-byte iterations ...
    }
}
```

This works because:

1. The analyzer's function prologue at the AST level doesn't do
   anything with the inline-asm block's contents — it's a raw
   text token.
2. The codegen's spill prologue copies `$04`/`$05` → the codegen
   local but **leaves the originals alone**. So the transport
   slots still hold the argument when the first instruction of
   the asm block executes.
3. None of the primitives `JSR` from inside the `asm { ... }`
   block, so nothing else re-enters the function's body (or any
   other function) while the inline block is running, which
   would re-populate `$04`/`$05` with different arguments.

The file has a big comment (`── Parameter convention ──`)
explaining exactly this. Every primitive in that file starts
with `LDX $04` (and if it has two params, `LDY $05`) instead of
`LDX {dst}` / `LDY {src}`.

### Once the compiler is fixed

Revert every `LDX $04` / `LDY $05` in `examples/sha256/sha_core.ne`
back to `LDX {dst}` / `LDY {src}` / `LDX {h_ofs}` / …, and delete
the "Parameter convention" comment. Also consider whether
`examples/inline_asm_demo.ne` should be updated so `times_four`
actually produces the documented `×4`, and regenerate
`tests/emulator/goldens/inline_asm_demo.png` in the same commit —
the current golden encodes the buggy behaviour.

### Guess at the fix

Two equivalent options, each about 10 lines of code:

**(a) Make the codegen use the analyzer's allocation for
locals.** Drop the `local_ram_next` loop at the top of
`Emitter::new` and, instead of minting new addresses, look up
each local's analyzer key and copy its address into
`var_addrs`:

```rust
for func in &ir.functions {
    for local in &func.locals {
        let qualified = /* __local__<scope>__<local.name> */;
        if let Some(a) = allocations.iter().find(|a| a.name == qualified) {
            var_addrs.insert(local.var_id, a.address);
            var_sizes.insert(local.var_id, a.size);
        }
    }
}
```

The analyzer already picks slots that are stable across
functions (the `__local__fn__name` prefix avoids collisions and
it allocates from zero page first, which is faster anyway), so
the codegen's "grow linearly from $0300" policy isn't actually
buying anything — and the comment in `ir_codegen.rs` explaining
why it's safe to stack locals was already relying on the same
"no recursion, bounded call depth" guarantees the analyzer
enforces. The analyzer's allocations already satisfy them.

**(b) Make `substitute_asm_vars` use the codegen's
`var_addrs`.** Pass `self.var_addrs` (plus the VarId map) into
the resolver instead of `self.allocations`. Same effect — both
maps agree after this — and arguably more local to the bug. The
analyzer's allocations stay as they are.

Preferred: (a) — it deletes code instead of rerouting it, and
it makes the memory map dumped by `--memory-map` truthful again
(the codegen's override was invisible to `--memory-map`, which
is why the discrepancy above looks puzzling without this writeup).

Once either change is in, re-run the full emulator harness. The
`inline_asm_demo` and `sha256` goldens will need fresh captures
because both change observable output.

---
