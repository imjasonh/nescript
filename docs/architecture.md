# Compiler Architecture

An overview of the NEScript compiler internals for contributors and maintainers.

---

## Pipeline

```
Source (.ne) --> Lexer --> Parser --> Analyzer --> IR Lowering --> Optimizer --> Codegen --> Assembler --> Linker --> ROM (.nes)
```

Each phase is a pure function (input to output) with no global state, making every stage independently testable.

```
.ne source ---------> Lexer -----> Parser -----> Analyzer ------>
                     (tokens)     (AST)       (annotated AST)

IR Lowering -----> Optimizer -----> Codegen -----> Linker -----> ROM
   (IR)          (optimized IR)   (6502 insns)               (.nes file)
```

### Phase Summary

| Phase           | Input               | Output                 | Responsibility                                       |
|-----------------|----------------------|------------------------|------------------------------------------------------|
| **Lexer**       | Source text           | Token stream           | Tokenization, number/string literal parsing          |
| **Parser**      | Token stream          | AST                    | Syntax validation, tree construction                 |
| **Analyzer**    | AST                   | Annotated AST          | Type checking, scope resolution, call graph analysis |
| **IR Lowering** | Annotated AST         | NEScript IR            | Flatten expressions, expand u16 ops, desugar         |
| **Optimizer**   | IR                    | Optimized IR           | Constant folding, dead code, ZP promotion, inlining  |
| **Codegen**     | Optimized IR          | 6502 instruction list  | Register allocation, instruction selection           |
| **Assembler**   | 6502 instructions     | Byte sequences + fixups| Opcode encoding, address resolution                  |
| **Linker**      | Bytes + assets        | .nes file              | Bank layout, vectors, iNES header                    |

---

## Modules

### `lexer/`
Tokenizes NEScript source text into a stream of typed tokens with source spans. Handles decimal, hex, and binary integer literals, string literals, all keywords, and operators.

### `parser/`
Recursive descent parser that converts the token stream into an AST. Defines all AST node types (expressions, statements, declarations) in `ast.rs`.

### `analyzer/`
Performs semantic analysis on the AST: type checking (`types.rs`), scope and symbol table management (`scope.rs`), and call graph construction with depth analysis (`call_graph.rs`). Detects recursion, type mismatches, undefined references, and call depth violations.

### `ir/`
Defines the intermediate representation and the lowering pass (`lowering.rs`) that translates the annotated AST into IR. Flattens nested expressions, expands 16-bit operations into 8-bit sequences, and resolves syntactic sugar.

### `optimizer/`
Runs optimization passes over the IR: constant folding (`const_fold.rs`), dead code elimination (`dead_code.rs`), zero-page promotion analysis (`zp_promote.rs`), and function inlining (`inliner.rs`).

### `codegen/`
Translates optimized IR into 6502 instructions. Includes register allocation for the A/X/Y registers (`regalloc.rs`) and instruction pattern matching for idiomatic 6502 code (`patterns.rs`).

### `asm/`
The built-in assembler. Encodes 6502 instructions (`encode.rs`) with all addressing modes (`addressing.rs`), using a complete opcode table (`opcodes.rs` -- 56 instructions across all modes).

### `linker/`
Assigns addresses to code and data segments, resolves fixups/relocations (`fixups.rs`), and handles bank allocation (`banks.rs`) for banked mappers.

### `rom/`
Builds the final iNES ROM file. Generates the 16-byte iNES header (`header.rs`) and places the NMI/RESET/IRQ vector table (`vectors.rs`).

### `runtime/`
Contains built-in runtime code that the compiler emits into every ROM: NES hardware initialization, NMI handler generation, controller read routines, OAM DMA setup, software multiply/divide, and the frame-walking audio driver (`gen_audio_tick`, `gen_period_table`, `gen_data_block`).

### `assets/`
The asset pipeline. Converts PNG images to CHR tile data (`chr.rs`), maps RGB colors to the NES palette (`palette.rs`), resolves `sprite`/`background` declarations into tile-indexed CHR blocks (`resolve.rs`), and compiles `sfx`/`music` declarations into ROM-ready envelope and note-stream byte tables (`audio.rs`) — plus the builtin effect/track tables used as fallbacks when programs reference audio names without declaring them.

### `debug/`
Debug instrumentation output. Generates source maps relating ROM addresses to source locations (`source_map.rs`), symbol tables compatible with Mesen (`symbols.rs`), and runtime check code for debug builds (`checks.rs`).

### `errors/`
Error reporting infrastructure. Defines the `Diagnostic` struct with error codes, severity levels, source spans, labels, help text, and notes (`diagnostic.rs`). Renders diagnostics with color and source context for terminal output (`render.rs`).

---

## Testing

### Test Organization

Tests are co-located with each module in `tests.rs` files:

```
src/lexer/tests.rs        -- lexer unit tests
src/parser/tests.rs       -- parser unit tests
src/analyzer/tests.rs     -- semantic analysis tests
src/ir/tests.rs           -- IR lowering tests
src/optimizer/tests.rs    -- optimizer tests
src/codegen/tests.rs      -- code generation tests
src/asm/tests.rs          -- assembler tests
src/linker/tests.rs       -- linker tests
src/rom/tests.rs          -- ROM builder tests
src/assets/tests.rs       -- asset pipeline tests
```

Integration tests live in the `tests/` directory:

```
tests/integration/        -- full pipeline tests with .ne source files
tests/error_tests/        -- tests that verify specific error codes
tests/asm_tests/          -- 6502 opcode and addressing mode tests
```

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test --lib lexer
cargo test --lib parser
cargo test --lib analyzer

# Run integration tests only
cargo test --test '*'

# Run a specific test by name
cargo test test_name
```

### Test Strategy

Each compiler phase is designed as a pure function, so unit tests provide isolated input and verify output without side effects. Integration tests compile complete `.ne` source files and verify the output ROM matches expected golden files in `tests/integration/expected/`.

Error tests in `tests/error_tests/` contain intentionally broken programs and verify that the correct error code is produced (e.g., `recursion.ne` should produce `E0402`).
