# NEScript v0.1 — Compiler Bugs and Limitations Found While Building War

This document captures bugs and limitations discovered while
building `examples/war.ne`. Each entry includes a minimal
reproduction, the symptom we observed, the root cause, the
workaround originally used in `examples/war/*.ne`, and the
compiler fix that shipped (when shipped).

## Status summary

| # | Short name | Status | Fix commit | Regression test |
|---|---|---|---|---|
| 1 | `fun` with > 4 params silently drops the rest | **FIXED** (E0506 diagnostic) | `analyzer: reject functions with more than 4 parameters (E0506)` | `analyze_rejects_function_with_more_than_4_params`, `analyze_accepts_function_with_exactly_4_params` |
| 1b | Same-named params share VarIds across functions | **FIXED** (scope-qualified keys) | `analyzer/ir: scope function locals per function body` | `analyze_allows_same_param_name_in_two_functions` |
| 2 | Param transport slots $04-$07 clobbered by nested calls | **FIXED** (codegen prologue spill) | `codegen: spill parameters from $04-$07 into per-function RAM slots` | `codegen::ir_codegen::gen_function_prologue_spills_params_to_local_ram` |
| 3 | Function-local `var` declarations share one flat namespace | **FIXED** (scope-qualified keys) | `analyzer/ir: scope function locals per function body` | `analyze_allows_same_local_name_in_two_functions`, `analyze_allows_same_local_name_in_two_state_handlers`, `analyze_still_rejects_duplicate_local_in_same_function` |
| 4 | 8-sprites-per-scanline limit invisible to user code | **FIXED** (W0109 static analyzer warning) | `analyzer: add W0109 sprite-per-scanline budget check` | `analyze_sprite_scanline_budget_warns_over_eight`, `analyze_sprite_scanline_budget_ok_when_staggered`, `analyze_sprite_scanline_budget_skips_dynamic_coords`, `analyze_sprite_scanline_budget_expands_metasprites`, `analyze_sprite_scanline_budget_recurses_into_if` |
| 5 | `inline` keyword silently declined for short functions | **FIXED** (IR lowering now inlines expression and void bodies) | `ir: real inlining for single-return and void-body `inline fun`s` | `ir::tests::inline_fun_expression_body_emits_no_call_at_use_site`, `inline_fun_void_body_statements_are_spliced`, `inline_fun_with_conditional_return_compiles_as_regular_call`, `inline_fun_nested_inlines_substitute_correctly` |
| 6 | `wide_hi` IR map leaked between functions (u16→u8 aliasing) | **FIXED** (cleared per function) | `ir: clear wide_hi between functions to fix 16-bit op aliasing` | `ir::tests::wide_hi_does_not_leak_between_functions` |

**Once a fix lands, revert the workaround in `examples/war/*.ne`
in the same commit** so the example keeps the game honest and
the PR diff visibly proves the fix works end-to-end. All seven
catalogued bugs have now shipped their fixes; the example code
no longer carries any workaround comments.

---

## 1. Functions with more than 4 parameters silently corrupt the 5th+ *(FIXED)*

### Symptom

Calling a function with 5 or 6 parameters compiles cleanly, with
no warning or error, but at runtime the 5th and 6th parameter
values are silently replaced by garbage (typically the value of
parameter 3 or 4). Animations and state writes that depend on
those parameters behave as if zero was passed.

### Reproduction

```nescript
fun arm_fly(sx: u8, sy: u8, dxsign: u8, dysign: u8, card: u8, fu: u8) {
    fly_x = sx
    fly_y = sy
    fly_dx_sign = dxsign
    fly_dy_sign = dysign
    fly_card = card        // gets the value of dxsign instead!
    fly_face_up = fu       // gets the value of dxsign instead!
}

fun caller() {
    arm_fly(32, 64, 0, 0, 147, 1)
    // After this call:
    //   fly_x = 32, fly_y = 64, fly_dx_sign = 0, fly_dy_sign = 0
    //   fly_card = 0   (NOT 147)
    //   fly_face_up = 0 (NOT 1)
}
```

### Root cause

`src/codegen/ir_codegen.rs` (around line 240) iterates through
`func.locals` and assigns the first 4 entries to zero-page
parameter slots `$04`-`$07`:

```rust
for func in &ir.functions {
    for (i, local) in func.locals.iter().enumerate() {
        if i < func.param_count {
            if i < 4 {
                var_addrs.insert(local.var_id, 0x04 + i as u16);
                ...
            }
        } else {
            ...
        }
    }
}
```

The `if i < 4` guard silently drops the mapping for params 5+
without inserting any RAM allocation for them. The corresponding
caller-side codegen for `Call` writes only the first four
arguments. Result: params 5 and 6 are never passed and the
callee reads stale memory from $04-$07 in their place.

### Workaround used in `examples/war/`

`arm_fly` is split: the four "arming" parameters stay in the
function signature, and `fly_card` / `fly_face_up` are written to
the global state directly at every call site instead. See
`war/play_state.ne` (`begin_draw_a` / `begin_draw_b`).

### Fix proposal

Two reasonable options:

1. **Diagnose-only**: emit `E05XX too many parameters` when a
   `fun` declaration has more than 4 params. This is the
   smallest possible change and turns silent miscompiles into a
   loud compile-time error. Should ship immediately even if
   option 2 is also planned.

2. **Spill to RAM**: extend the calling convention so params
   beyond the first four are passed via dedicated RAM slots in
   the callee's local frame. The caller-side `Call` codegen
   would write those slots before `JSR`, the callee-side prologue
   could leave them as-is. This grows the per-function RAM
   footprint but lets users write any signature they like.

---

## 1b. Function parameters with the same name in different functions share a VarId, which collides their zero-page slot mapping *(FIXED)*

### Symptom

Two unrelated functions whose parameters happen to be named the
same (e.g. both have a `card: u8` parameter, or both have an
`x: u8` parameter) end up reading parameters from the wrong
zero-page slot at runtime. One function reads `$04`, another
reads `$06`, a third reads `$05` — depending on the parameter's
*position* in whichever function is processed last by the
codegen.

This is a much sneakier sibling of bug #1: rather than dropping
a parameter past the 4th slot, it silently reroutes parameter
reads to slots that hold completely unrelated values from the
caller.

### Reproduction

```nescript
// Function A: card is the 1st parameter, expected at $04
fun push_back_a(card: u8) {
    deck_a[deck_a_front] = card   // reads from $06, not $04!
    deck_a_count += 1
}

// Function B: card is the 3rd parameter, expected at $06
fun draw_card_face(x: u8, y: u8, card: u8) {
    // ... uses card normally ...
}
```

The IR lowering assigns `card` a single shared `VarId` because
its `var_map` is global across all functions. The codegen then
walks each function in turn, inserting `(VarId(card), $0X)`
mappings into a single global `var_addrs` `HashMap` — and
whichever function comes last in iteration order wins the
mapping. If `draw_card_face` is processed after `push_back_a`,
`VarId(card)` ends up mapped to `$06`, and `push_back_a` then
reads its `card` parameter from `$06` (which holds whatever the
caller was using as a third argument — typically junk).

### Root cause

`src/ir/lowering.rs::get_or_create_var` looks up names in
`self.var_map`, which is shared across the whole program:

```rust
fn get_or_create_var(&mut self, name: &str) -> VarId {
    if let Some(&id) = self.var_map.get(name) {
        id
    } else {
        let id = VarId(self.next_var_id);
        self.next_var_id += 1;
        self.var_map.insert(name.to_string(), id);
        id
    }
}
```

`lower_function` calls `get_or_create_var(&param.name)` for each
parameter, so two different functions both with a `card`
parameter resolve to the same `VarId`. Once that single `VarId`
flows into the codegen, the per-function "this is param index N
of function F" relationship is lost — there's only one global
mapping per `VarId`.

### Workaround used in `examples/war/`

Every parameter name in the war source is unique across the
entire program. Function-locals were already prefixed by
function (see bug #3); we extended the same scheme to params:
`push_back_a(pba_arg_card: u8)` instead of
`push_back_a(card: u8)`, etc. The wrapping `pba_card` /
`pbb_card` / `dcf_card` snapshots from bug #2 stay because they
also help with the bug-2 clobbering.

### Fix

Both the analyzer and the IR lowerer now qualify function-body
`var` / parameter declarations with the enclosing function name
(or state handler name) under an internal key
`"__local__{scope}__{name}"`. Each function's locals and
parameters therefore get **distinct** symbol-table entries and
VarIds even when the source names collide.

Lookups inside a function body go through
`Analyzer::resolve_symbol` / `LoweringContext::scoped_key`,
which prefer the scope-qualified key over the bare one — so
a function-local `var x` correctly shadows a same-named global
(or another function's `var x`).

State-level locals (declared at `state Foo { var x: u8 }`
outside any handler) stay in the global namespace so every
handler in the state can read/write them across frames.

See `src/analyzer/mod.rs::resolve_symbol` / `resolve_key` /
`scoped_name` and `src/ir/lowering.rs::scoped_key`.

Together with fix #2 below, bugs #1b and #2 are completely
gone: the workaround-prefixed locals and params in `war/*.ne`
(the `dcf_`, `dwp_`, `pba_`, etc tags) are all reverted.

---

## 2. Function parameters share zero-page slots with nested calls — values clobbered across `JSR` *(FIXED)*

### Symptom

A function that takes parameters and then calls another function
sees its own parameters silently replaced by the inner call's
arguments. Any code path that reads the original parameter
*after* the inner call gets the wrong value.

### Reproduction

```nescript
fun draw_card_face(x: u8, y: u8, card: u8) {
    var rank: u8 = card_rank(card)   // x at $04 is now `card`
    var suit: u8 = card_suit(card)   // x at $04 is still `card`
    // x is supposed to be 120 here, but it's actually `card`
    var x1: u8 = x + 8               // computes card + 8, not 120 + 8
    draw Tileset at: (x, y) frame: ...   // draws at x = card, not 120
}
```

Concretely, calling `draw_card_face(120, 128, 0x93)` puts the
card sprite at `(0x93, 128)` — completely wrong.

### Root cause

Same allocator as bug #1: `func.locals[0..param_count]` are
mapped to `$04`, `$05`, `$06`, `$07`. The caller writes its own
arguments into the same zero-page slots before `JSR`, so the
caller's parameters at those slots get clobbered by the callee's
arguments. There is no save/restore wrapper around `JSR` and no
spill/reload pass to refresh the caller's parameters from a
backing copy.

### Workaround used in `examples/war/`

Every helper that takes parameters AND makes any nested function
call snapshots its parameters into fresh local variables at the
top of the function, then references the locals exclusively
throughout the body. See `war/render.ne::draw_card_face`,
`war/render.ne::draw_flying_card`, `war/deck.ne::push_back_a`,
`war/deck.ne::push_back_b`.

### Fix

`codegen::ir_codegen::IrCodeGen::new` now allocates every
function-local — including its parameters — into a dedicated
per-function RAM slot at `$0300+`. Parameters are still passed
via the zero-page transport slots `$04-$07` as the calling
convention, but `gen_function` now emits a 4-instruction
**prologue** at every function entry:

```
LDA $04         ; transport slot 0
STA <param_0_addr>
LDA $05         ; transport slot 1
STA <param_1_addr>
... etc ...
```

By the time the body runs, every parameter lives in the
function's dedicated RAM slot, so any nested call can freely
clobber `$04-$07` (passing its own arguments to _its_ callee)
without corrupting the caller's saved parameters.

The cost is 4 LDA/STA pairs at every function entry (≈ 20
bytes of ROM, 16 cycles). Worth it to make the calling
convention sound.

See `codegen::ir_codegen::gen_function_prologue_spills_params_to_local_ram`
for the regression test.

---

## 3. Function-local variable names are in a flat global namespace *(FIXED)*

### Symptom

Two different functions cannot declare locals with the same
name. The compiler emits `E0501 duplicate declaration of '<name>'`
even though the locals are in disjoint scopes.

### Reproduction

```nescript
fun foo() {
    var i: u8 = 0
    while i < 10 { i += 1 }
}

fun bar() {
    var i: u8 = 0   // E0501 duplicate declaration of 'i'
    while i < 5 { i += 1 }
}
```

### Root cause

`src/analyzer/mod.rs::register_var` inserts every `var`
declaration into a single `self.symbols` map keyed only on the
variable's name, with no qualification by function or block:

```rust
fn register_var(&mut self, var: &VarDecl) {
    if self.symbols.contains_key(&var.name) {
        self.diagnostics.push(Diagnostic::error(
            ErrorCode::E0501,
            format!("duplicate declaration of '{}'", var.name),
            var.span,
        ));
        return;
    }
    ...
}
```

`check_statement` calls `register_var` for every `Statement::VarDecl`
encountered while walking function bodies, so all locals across
all functions and all nested blocks land in the same namespace.

### Workaround used in `examples/war/`

Every function-local variable is prefixed with a short tag
identifying its enclosing function (e.g. `dfa_card` in
`draw_front_a`, `pba_slot` in `push_back_a`,
`dwp_px` in `draw_word_player`). This makes long files harder to
read but is fully mechanical.

### Fix

Same as #1b: the analyzer and IR lowerer now internally
qualify function-body `var` declarations with the enclosing
scope's name, so `foo`'s `var i` and `bar`'s `var i` resolve
to `__local__foo__i` and `__local__bar__i` respectively. The
two entries coexist peacefully in the (still-flat) symbol
table.

What *didn't* change: two `var i` declarations inside the
same function body still collide with E0501 (we scoped per
function body, not per nested block). That's a deliberate
trade-off — per-block scoping would require live-range
analysis to reuse RAM slots across blocks, which is a much
bigger change. The analyzer test
`analyze_still_rejects_duplicate_local_in_same_function`
pins this behaviour.

---

## 4. Per-frame sprite-per-scanline limit is invisible to user code *(FIXED)*

### Symptom

Drawing more than 8 sprites whose Y rectangles intersect a
single scanline causes the NES PPU to silently drop the excess
sprites past the 8th in OAM order. Letters or tiles just don't
render, and prior to this fix the compiler emitted no warning
even when the entire layout was a tree of literal coordinates
it could have checked.

### Reproduction

```nescript
// 9 letters all on the same Y row:
draw Letter at: (0,  100)
draw Letter at: (8,  100)
draw Letter at: (16, 100)
draw Letter at: (24, 100)
draw Letter at: (32, 100)
draw Letter at: (40, 100)
draw Letter at: (48, 100)
draw Letter at: (56, 100)
draw Letter at: (64, 100)  // past budget — silently dropped
```

Pre-fix the compiler said nothing and the 9th letter never
showed up on hardware. Post-fix the analyzer emits:

```
warning[W0109]: state 'Main' draws 9 literal-coordinate sprites
               overlapping scanline 100; the NES renders at
               most 8 sprites per scanline
 = help: stagger draws vertically by at least 8 pixels, reduce
   the number of on-screen sprites, or split the draws across
   `on_scanline` handlers
 = note: the 9th and later sprites on a scanline are dropped
   by the PPU, causing flicker or invisible objects on real
   hardware
```

### Root cause

The 8-sprites-per-scanline cap is a real NES hardware
constraint, not a compiler bug — but NEScript had no static
check to catch the cases where user code makes the problem
obvious at compile time, even though the draw allocator is
sequential and the literal coords it sees are trivially
checkable.

### Workaround used in `examples/war/`

We staggered text rows by hand. The title screen's "WAR /
CARD GAME / 0 PLAYER / 1 PLAYER / 2 PLAYER" layout sits each
row at a different y so no scanline carries more than 7
sprites; the victory screen's "PLAYER X / WINS" wraps after
the player letter for the same reason. These layouts stay in
place post-fix — they now pass the analyzer cleanly because
they're under budget.

### Fix

`src/analyzer/mod.rs::check_sprite_scanline_budget` runs at
the end of `analyze_program`. For each state's `on_frame`
handler it walks the block tree (including nested
`if`/`while`/`for`/`loop`) collecting literal-coordinate
`draw` statements into a `Vec<(y, x, span)>`. Metasprite
draws expand into one tuple per tile via the metasprite's
`dx`/`dy` offset arrays, so a metasprite that covers four
tiles on the same y contributes four sprites to the overlap
count. Non-literal coordinates are skipped entirely because
the static analysis can't know where they land at runtime.

With the tuples collected, the analyzer iterates every
scanline 0..240 and counts sprites whose `y <= scanline <
y+8`. The worst scanline is cached and, if the count exceeds
8, a `W0109` diagnostic is emitted with labels pointing at
every draw site that contributed (deduplicated so metasprite
expansions don't spam the message).

Only `on_frame` is checked. `on_enter` / `on_exit` fire once
per transition and aren't the hot sprite path; checking them
would produce false positives on brief splash animations.
Conditional branches are unioned (conservative over-count) —
a sprite drawn inside an `if` counts for budget purposes even
if its runtime branch is exclusive with a sibling's. The
trade-off: the check stays local and simple, at the cost of
occasionally flagging hand-sliced layouts that the user knows
are actually safe.

### Regression tests

Five tests in `src/analyzer/tests.rs`:

- `analyze_sprite_scanline_budget_warns_over_eight` — nine
  literal draws on the same `y` trips W0109.
- `analyze_sprite_scanline_budget_ok_when_staggered` — nine
  draws each on a different `y` row are silent.
- `analyze_sprite_scanline_budget_skips_dynamic_coords` —
  draws with a `var`-backed `x` are skipped (no false
  positive) because the analysis can't resolve them.
- `analyze_sprite_scanline_budget_expands_metasprites` — a
  four-tile metasprite drawn three times trips W0109 because
  the analyzer expands each draw into its per-tile offsets.
- `analyze_sprite_scanline_budget_recurses_into_if` — nine
  draws inside an `if` block still trip W0109 (conservative
  over-count).

### Layer-2: runtime sprite cycling (`cycle_sprites`)

W0109 only catches the literal-coordinate case — a game with
>8 dynamically-positioned sprites (enemies, projectiles,
animated NPCs) is invisible to it. The hardware will still
drop the 9th+ sprite on every frame, and because draw order
is stable frame-to-frame the *same* sprite goes missing every
frame, which reads to the developer as a game bug rather
than a hardware limit.

The classic NES mitigation is sprite cycling: rotate the OAM
DMA start offset each frame so different sprites land in the
PPU's "first 8" on each successive frame. Over N frames (where
N is the number of overlapping sprites) each sprite gets
dropped exactly once, and the eye reconstructs the missing
pixels from frame persistence. Permanent dropout becomes
visible flicker — the failure mode every NES player
recognises, and vastly better UX than "my bullet disappeared."

NEScript ships this as the opt-in `cycle_sprites` statement:

```nescript
on frame {
    draw Enemy0 at: (e0x, e0y)
    draw Enemy1 at: (e1x, e1y)
    // ...lots of enemies...
    cycle_sprites   // bump the rotating offset one slot
    wait_frame
}
```

Each call adds 4 to a one-byte runtime counter at `$07EF`
(natural u8 wrap at 256 → 0) and emits a `__sprite_cycle_used`
marker label. The linker reads the marker and swaps the NMI
handler over to a variant that writes the counter to `$2003`
before triggering the OAM DMA, so each frame's DMA lands in
a different slot of the PPU's OAM buffer. Over 64 frames the
rotation completes a full cycle.

Programs that don't call `cycle_sprites` emit no marker and
get the original fixed-offset NMI path, so every existing
golden ROM stays byte-identical. Opt-in by design — the
tradeoff is "cosmetic HUD elements you pinned to slot 0 lose
their pin" — so programs that manage OAM priority manually
can keep doing so.

The [`examples/sprite_flicker_demo.ne`](../sprite_flicker_demo.ne)
example drives 12 sprites into a 4-pixel band to exercise
both W0109 at compile time and `cycle_sprites` at runtime;
the committed emulator golden captures a specific frame of
the cycling pattern.

### Layer-3: debug-mode runtime overflow telemetry

`debug.sprite_overflow_count()` and `debug.sprite_overflow()`
mirror the existing `debug.frame_overrun_count()` /
`debug.frame_overran()` pair. In debug builds the NMI handler
samples the PPU's sprite-overflow flag (`$2002` bit 5) at the
top of vblank — it reflects whether any scanline of the
just-finished frame had more than 8 sprites and fired the
hardware "give up" pathway. If the bit is set the handler
bumps a cumulative counter at `$07FD` and sets a per-frame
sticky bit at `$07FC`, which the next `wait_frame` clears.

User code reads those bytes via the new builtins:

```nescript
debug.assert(not debug.sprite_overflow())
```

…or, in an overlay:

```nescript
var ovf: u8 = 0
ovf = debug.sprite_overflow_count()
draw Digit at: (8, 8) frame: ovf
```

The PPU hardware flag has well-known quirks (it occasionally
misses the 9th sprite or sets the flag when none actually
overflowed), but it's correct for the overwhelming majority
of cases and is essentially free to sample — one `LDA $2002;
AND #$20` at NMI top, ~15 cycles per frame. Release builds
never emit the check block, so the four bytes at `$07EF` /
`$07FC`-`$07FD` remain free for the analyzer to allocate.

Combined, the three layers catch the sprite-per-scanline
limit at three different lifecycle stages: W0109 at compile
time for statically-knowable layouts, `debug.sprite_overflow*`
at playtest time for the dynamic cases W0109 can't see, and
`cycle_sprites` at runtime as a graceful fallback for the
cases the user knows are unavoidable.

---

## 5. The `inline` keyword is a hint and is silently ignored for short functions *(FIXED)*

### Symptom

Marking a tiny function `inline fun` did not inline it.
The compiler still emitted a real `JSR` with full parameter
passing through `$04`-`$07`, which meant the declared-inline
helpers in War (`card_rank`, `card_suit`, `set_phase`) still
paid the calling-convention overhead and still fell foul of
the bug-2 clobbering until the param-spill prologue landed.

### Reproduction

```nescript
inline fun card_rank(card: u8) -> u8 {
    return card >> 4
}
```

Pre-fix, the asm dump showed `JSR __ir_fn_card_rank` at every
call site. Post-fix the body is spliced at each use and no
`JSR` is emitted at all.

### Root cause

The IR lowerer's old handling of `inline fun` was a no-op —
`is_inline` was read off the AST but the lowering path for
`Call` never branched on it. The optimizer passes also had
no inlining transform. So the keyword was parsed and then
dropped on the floor, producing regular out-of-line code.

### Fix

`src/ir/lowering.rs` now captures inline bodies up front in
`LoweringContext::capture_inline_bodies` and rewrites call
sites at lowering time. Two body shapes are supported:

1. **Single-return expression** (e.g. `return card >> 4`) —
   captured as `InlineBody::Expression(Expr)`. At the call
   site, the lowerer evaluates each argument into a fresh
   temp, pushes a substitution frame mapping parameter names
   to those temps, and recursively lowers the expression in
   place of a `Call` op. No IR `Call`/`Return` ops are
   emitted; the caller ends up with the same IR it would
   have had if the expression were written directly.

2. **Void multi-statement body** — captured as
   `InlineBody::Void(Vec<Statement>)`, but only when every
   statement passes `is_splicable_void_stmt` (plain
   assignments, statement-level calls, draws, palette/
   background/scroll writes, `wait_frame`, inline asm, debug
   builtins). Any control flow (`if`/`while`/`for`/`loop`/
   `return`/`break`/`continue`/`transition`) disqualifies
   the function from being inlined, and the call stays a
   regular `Call`. This mirrors War's `set_phase` (a
   four-statement global assign) and `reset_flight` (a
   similar pattern).

Functions that are marked `inline` but have a body shape the
simple substitution machinery can't splice — notably ones
with conditional early returns like War's `wrap52` — fall
back to regular out-of-line calls with no diagnostic. That's
a deliberate trade-off: rather than refuse to compile the
program or emit a noisy warning, we degrade gracefully. The
`inline` keyword is now a best-effort hint whose "best
effort" is predictable and documented here.

### Substitution stack

Nested inline expansions push a fresh substitution frame so
an inline body calling another inline sees the inner
function's parameter substitutions, not its own.
`lookup_inline_sub` walks only the top of the stack because
inner bodies are lowered to completion before the stack is
popped, so an unambiguous "current" frame always exists. See
`LoweringContext::inline_subs_stack` and
`lower_expr::Expr::Ident` (which checks the substitution
stack before the global var table).

### Regression tests

Four tests in `src/ir/tests.rs`:

- `inline_fun_expression_body_emits_no_call_at_use_site` —
  a `return x * 2` inline emits no `Call`, just the multiply.
- `inline_fun_void_body_statements_are_spliced` — a void
  three-statement inline compiles to three individual ops
  at the caller, not a `Call`.
- `inline_fun_with_conditional_return_compiles_as_regular_call`
  — a body with an `if ... return` pattern falls back to a
  regular `Call` op.
- `inline_fun_nested_inlines_substitute_correctly` — inline A
  calling inline B sees B's parameter substitutions, not A's.

---

## 6. `wide_hi` IR-lowering map leaked between functions and corrupted 16-bit ops *(FIXED)*

### Symptom

A function whose body had no 16-bit values whatsoever would
nonetheless emit `CmpEq16` (and other `Op16` variants) where the
*destination* temp aliased one of the *source* temps. The
resulting comparison effectively became "is this byte equal to
some uninitialised stack memory?", which in War caused the
phase-machine `match phase { ... }` dispatcher to skip the
`P_WIN_B` arm forever once the game first reached it — the game
would freeze with both cards face-up and "PLAYER B WINS" never
firing.

### Reproduction (pre-fix)

A handful of `u16` `+= 1` operations early in a state handler
followed by a long `match` chain on a `u8` was enough to trip it.
The minimum repro is roughly:

```nescript
var clock: u16 = 0
var phase: u8 = 0
on frame {
    clock += 1                    // wide op leaves wide_hi entries
    match phase {                 // u8 match — should be 8-bit
        0 => { phase = 1 }
        1 => { phase = 2 }
        2 => { phase = 3 }
        3 => { phase = 4 }
        4 => { phase = 5 }
        5 => { phase = 6 }
        6 => { phase = 7 }
        7 => { /* corrupt — never matched */ }
        _ => {}
    }
}
```

The IR for the `phase == 7` arm came out as
`CmpEq16 { dest: T147, a_lo: T145, a_hi: T148, b_lo: T146,
b_hi: T147 }` — note `dest == b_hi`. The codegen happily emits
the corresponding 16-bit asm, but reads garbage for the `b_hi`
operand because it points at the same scratch slot the result
will be written to.

### Root cause

`src/ir/lowering.rs::IrLowerer` carries a `wide_hi: HashMap<IrTemp, IrTemp>`
that records "this low temp's high byte lives at this other
temp" pairs whenever a 16-bit value is produced. `lower_function`
and `lower_handler` both reset `next_temp = 0` at the start of
each function — but they did *not* clear `wide_hi`. Stale entries
from earlier functions stuck around and matched against fresh
temp IDs in subsequent functions (which start counting from 0
again), causing `is_wide(t)` and `widen(t)` to return spurious
"wide" results for what should have been narrow `u8` values.

When that happens inside `lower_binop`'s `Eq` path, `widen(r)`
returns the stale `(r, hi_r)` pair where `hi_r` happens to be the
*next* temp ID `fresh_temp()` will hand out a moment later — so
the `dest` temp and `b_hi` end up identical.

### Fix

`src/ir/lowering.rs`: in both `lower_function` and `lower_handler`,
add `self.wide_hi.clear();` immediately after `self.next_temp = 0;`.
Done in this PR.

### Why this didn't show up sooner

Every prior example either declared no `u16` globals at all, or
declared one and used it sparingly enough that the temp IDs
the leaked entries claimed never collided with the rest of the
function. War is the first example that combines a `u16`
free-running counter with a deep state machine that does many
`u8` comparisons in the same `on frame` body, which is exactly
the shape the bug needs to manifest.

### Regression test

`src/ir/tests.rs::wide_hi_does_not_leak_between_functions` (added
in this PR) compiles a two-function program where function A
uses a `u16 += 1` (creating wide entries) and function B does
`u8 == const` comparisons in a match. Pre-fix, the IR would emit
`CmpEq16` with aliased dest/source; post-fix it emits the
expected 8-bit `CmpEq`.

---

## Verification path after fixes

Once any of the bugs above are fixed in the compiler, the
corresponding workarounds in `examples/war/*.ne` should be
reverted in the same PR so:

- The example demonstrates idiomatic code, not workaround code.
- The PR's diff visibly proves the fix works end-to-end (the
  workaround removal would otherwise be a silent regression).
- The committed `examples/war.nes` rebuilds byte-identically to
  the reverted source, which the pre-commit hook enforces.

The relevant workaround sites are catalogued in each bug's
"Workaround used" section above; grep for the prefix tags
(`dcf_`, `dfa_`, `pba_`, `dwp_`, …) to find them all.
