# NEScript Decompiler Guide

The decompiler converts existing .nes ROMs back into editable NEScript `.ne` source code, enabling round-trip workflows: decompile an existing game, edit constants/audio/sprites, then recompile.

## Quick Start

```bash
# Decompile a ROM
nescript decompile game.nes -o game.ne

# Edit the source
vim game.ne  # Change physics constants, music, sprites, etc.

# Recompile
nescript build game.ne -o game_modified.nes

# Verify in the emulator (optional)
# Load game_modified.nes in jsnes or your NES emulator
```

## How It Works

The decompiler uses a **hybrid-shim** approach:

1. **ROM Parsing:** Reads the iNES header to extract mapper, mirroring, PRG/CHR bank counts
2. **Asset Extraction:** Identifies palettes, nametables, audio drivers and extracts them as structured data
3. **Binary Pass-Through:** Raw machine code passes through verbatim as `raw_bank` declarations (no 6502 decompilation)
4. **Source Generation:** Emits a valid `.ne` file with:
   - `game` declaration (mapper, mirroring)
   - `raw_bank` declarations for PRG/CHR code
   - Structured declarations for lifted assets (palettes, backgrounds, audio)

The output is **address-pinned**: every asset knows its original ROM byte offset, so recompilation can:
- Place the asset back in its original slot (if unchanged)
- Relocate it to free space (if the edit grew larger)
- This preserves ROM compatibility and avoids address shifts

## Supported ROM Types

The decompiler works best with **NEScript-produced ROMs** (where fingerprinting succeeds). For other ROMs:
- Safe fallback: full identity pass-through as `raw_bank` declarations
- Edit `game { mapper, mirroring }` and recompile unchanged
- Edit the binary declarations if you understand the bank layout

## Use Cases

### Modding Existing Games (NEScript-Produced)

```nescript
// Original ROM decompiled to game_orig.ne
// Edit constants:
const PLAYER_SPEED: u8 = 4  // was 2
const JUMP_HEIGHT: i8 = -6  // was -4

// Edit music/sfx (if lifted)
music Battle { notes: [C5 4, E5 4, G5 8, ...] }

// Recompile
nescript build game_orig.ne -o game_harder.nes
```

### Reverse-Engineering Gameplay

```bash
# Decompile to understand ROM structure
nescript decompile original_game.nes -o analysis.ne

# The raw_bank declarations show which PRG banks contain code
# The structured declarations show lifted assets (palettes, audio)
# Examine the emitted source to understand the game's ROM layout
```

### Round-Trip Testing

```bash
# Verify decompile-recompile cycle (for compiler regression testing)
nescript decompile rom.nes -o decomp.ne
nescript build decomp.ne -o rom_recompiled.nes

# If ROM bytes match byte-for-byte, the cycle is identity
# If they differ, check future-work.md for pending language features
```

## What Gets Decompiled

### Extracted (Structured Declarations)

- ✅ **Mapper**: Recognized from iNES header
- ✅ **Mirroring**: Horizontal/vertical from header
- ✅ **CHR Data**: Binary dump (conversion to PNG pending)
- ⏳ **Palette Data**: Recognition and structured emission (pending)
- ⏳ **Background Nametables**: Recognition and structured emission (pending)
- ⏳ **Audio Drivers**: FamiTone2 period table detected; data extraction pending

### Passed Through (Opaque raw_bank)

- ✅ **PRG Code**: All CPU code (6502 machine code)
- ✅ **Audio Driver Code**: FamiTone2 or other drivers

The decompiler **does not** perform 6502 disassembly or structured code lifting; this preserves original behavior and avoids the complexity of full-strength decompilation.

## Limitations

### Language Feature Dependencies

The decompiler relies on NEScript language features to express decompiled output:

| Feature | Status | Impact |
|---------|--------|--------|
| Address-pinned declarations (`@ 0xADDR`) | Pending M1 | Can't pin assets to original offsets; relocation workarounds needed |
| raw_bank pass-through | Pending M1 | Can't emit opaque code banks; full ROM reconstruction blocked |
| raw_vectors opt-out | Pending M1 | Can't preserve custom reset/NMI/IRQ addresses |
| Structured audio declarations | Pending M4 | Can't emit music/sfx as declarations; audio stays as binary |
| goto/label escape hatch | Pending M2 | Can't decompile unstructured 6502 code (rarely needed for games) |

See `docs/future-work.md` **Decompilation support** section for progress on these.

### ROM Types

**Well-supported:**
- NEScript-produced ROMs (all mapper types)
- Simple 8x8 sprite-based games with standard audio

**Best-effort:**
- Third-party ROMs with unknown drivers (identity pass-through)
- Banked ROMs (raw code pass-through; no cross-bank call analysis)

**Not supported:**
- Per-scanline PPU timing tricks (palette cycling, split-screen)
- Custom interrupt handlers (decompiler assumes NEScript runtime)
- Compressed or self-modifying code (no decompression or symbolic analysis)
- Games with non-standard memory layouts

## CLI Reference

### Decompile a ROM

```bash
nescript decompile <rom.nes> [-o output.ne]
```

**Arguments:**
- `<INPUT>` (required): Path to the `.nes` file to decompile

**Options:**
- `-o, --output <PATH>`: Output `.ne` file (default: input with `.ne` extension)

**Example:**
```bash
# Decompile to hello.ne (default)
nescript decompile hello.nes

# Decompile to custom path
nescript decompile game.nes -o custom_name.ne
```

## Integration with the Compiler

The decompiled `.ne` source is a **valid NEScript program** — compile it immediately:

```bash
# Decompile
nescript decompile original.nes -o decomp.ne

# Recompile (should produce byte-identical ROM if no edits)
nescript build decomp.ne -o recompiled.nes

# Compare (Unix/Linux/Mac)
diff original.nes recompiled.nes  # If empty, byte-identical ✓

# Verify with emulator harness
cd tests/emulator && npm install && node run_examples.mjs  # Pixel/audio comparison
```

## Development Status

The decompiler is **in active development** as part of the NEScript Milestone Plan:

- ✅ **M3:** ROM parsing, identity pass-through, decompile CLI
- ✅ **M4:** FamiTone2 audio driver detection
- ⏳ **M1:** Language foundations (address-pinned, raw_bank)
- ⏳ **M2:** goto/label support (for unstructured code)
- ⏳ **M5:** Round-trip integration tests

Once M1-M2 land, the decompiler will support full address-pinned asset edits with byte-exact recompilation.

## Troubleshooting

### "Error: failed to read ROM file"

Check that the path is correct and the file is readable:
```bash
ls -la game.nes  # Verify file exists
file game.nes    # Confirm it's an iNES ROM (should show "data")
```

### "Error: ROM too small" or "Invalid iNES header"

The file is not a valid iNES ROM. Check:
- It's not a compressed archive (`.zip`, `.7z`)
- It's not a different ROM format (NES 2.0 is supported, but older formats may not be)
- It's not corrupted (try decompiling on another machine)

### Decompiled ROM won't compile

The decompiled `.ne` file has syntax errors or references undefined symbols. Check:
```bash
nescript check decomp.ne  # Type-check without compiling
```

Look for error messages and fix the `.ne` source. If the error is about address-pinned declarations or raw_bank, those features are pending (see **Language Feature Dependencies** above).

### Recompiled ROM is byte-different

This is normal during development (pending full M1 implementation). The recompiled ROM is still **emulator-compatible** — verify with the jsnes harness:

```bash
# If available in CI or locally:
cd tests/emulator && UPDATE_GOLDENS=0 node run_examples.mjs
```

If the emulator golden (pixel/audio) matches the original, the ROM is correct; byte differences are acceptable.

## Next Steps

- **Want to edit a game?** Decompile it, edit constants in the `.ne` file, recompile
- **Want to contribute?** See `docs/future-work.md` for pending features (M1-M5)
- **Have feedback?** Open an issue on GitHub

---

**See Also:**
- `docs/language-guide.md` — NEScript language reference
- `docs/future-work.md` — Decompiler roadmap
- `docs/architecture.md` — Compiler pipeline overview
- `examples/` — Decompile any example to see decompiler output
