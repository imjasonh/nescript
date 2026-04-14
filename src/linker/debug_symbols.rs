//! Debug-symbol file writers.
//!
//! Produces Mesen-compatible `.mlb` label listings and plain-text
//! source maps from a [`LinkedRom`]. These helpers are owned by the
//! linker because they're the only place in the compiler that can
//! observe the final CPU address of each label — and the CPU-to-ROM
//! offset math needs the `fixed_bank_file_offset` the linker hands
//! back.
//!
//! Callers: `src/main.rs` invokes [`render_mlb`] when the user
//! passes `--symbols <path>` and [`render_source_map`] when the
//! user passes `--source-map <path>`. The functions themselves are
//! pure string producers so that unit tests can round-trip them
//! without touching the filesystem.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::LinkedRom;
use crate::analyzer::VarAllocation;
use crate::lexer::Span;

/// Render a Mesen-compatible `.mlb` symbol file from a
/// [`LinkedRom`].
///
/// Each line has the form `<type>:<hex-address>:<label>`. The type
/// byte follows Mesen's convention: `P` for PRG ROM offsets and
/// `R` for RAM (zero page and internal RAM share this namespace
/// because zero page is just the low 256 bytes of RAM on the NES).
///
/// Function and state-handler labels are emitted as `P` entries
/// with the address converted from a CPU address in `$C000-$FFFF`
/// into a PRG-relative ROM offset via
/// `linked.fixed_bank_file_offset`. User variables come from the
/// analyzer's `var_allocations` list and are emitted as `R`
/// entries at their assigned RAM addresses.
///
/// Internal labels (anything that doesn't look like a user
/// function or a well-known entry point) are skipped so the
/// resulting file focuses on the symbols a debugger user actually
/// cares about. Output is sorted deterministically so the file is
/// diff-friendly and so tests can assert against exact strings.
#[must_use]
pub fn render_mlb(linked: &LinkedRom, var_allocations: &[VarAllocation]) -> String {
    let mut out = String::new();

    let sorted: BTreeMap<&String, &u16> = linked.labels.iter().collect();
    let base_cpu_addr: u16 = 0xC000;
    for (label, &&cpu_addr) in &sorted {
        // Only translate labels that sit inside the fixed bank's
        // CPU window. In practice every label in `linked.labels`
        // already lives here (the assembler works on a single
        // bank at a time), but we guard anyway so a future
        // multi-bank label dump doesn't silently emit garbage
        // offsets.
        if cpu_addr < base_cpu_addr {
            continue;
        }
        let Some(display_name) = mlb_symbol_name(label) else {
            continue;
        };
        let rom_offset = linked.fixed_bank_file_offset + (cpu_addr - base_cpu_addr) as usize;
        // Mesen uses ROM file offsets *relative to the start of
        // the PRG region* (i.e. subtract the 16-byte header).
        // This keeps `.mlb` files portable between NES 2.0 and
        // iNES 1 headers.
        let prg_offset = rom_offset.saturating_sub(16);
        let _ = writeln!(out, "P:{prg_offset:04X}:{display_name}");
    }

    // Variables — emit in address order so the file is easy to
    // eyeball and diff. Duplicate names (e.g. struct fields under
    // two synthetic entries) are rare; when they do occur we keep
    // the first encounter.
    let mut vars: Vec<&VarAllocation> = var_allocations.iter().collect();
    vars.sort_by_key(|a| a.address);
    for var in vars {
        let _ = writeln!(out, "R:{:04X}:{}", var.address, var.name);
    }

    out
}

/// Determine whether a label should appear in the `.mlb` symbol
/// table, and if so under what name. Returns `None` for labels
/// that are internal bookkeeping (branch/skip stubs, temporary
/// jump targets) and wouldn't help a user navigate the ROM.
fn mlb_symbol_name(label: &str) -> Option<String> {
    // Function/state entry labels — strip the `__ir_fn_` prefix so
    // Mesen displays the user-facing name. `__ir_fn_Main_frame`
    // becomes `Main_frame`, for example.
    if let Some(rest) = label.strip_prefix("__ir_fn_") {
        return Some(rest.to_string());
    }
    // Bank trampolines and the main loop entry are useful entry
    // points for a reverse-engineering session.
    if label == "__ir_main_loop" {
        return Some("main_loop".to_string());
    }
    if let Some(rest) = label.strip_prefix("__tramp_") {
        return Some(format!("tramp_{rest}"));
    }
    // Hardware entry points. These are always present and
    // useful.
    if matches!(label, "__reset" | "__nmi" | "__irq" | "__irq_user") {
        return Some(label.trim_start_matches('_').to_string());
    }
    // Source-map markers are written to their own file via
    // `render_source_map`, not here.
    if label.starts_with("__src_") {
        return None;
    }
    // Everything else — per-block labels, fallthrough skips,
    // compiler-private helpers — gets filtered out to keep the
    // symbol file short and navigable.
    None
}

/// Render a plain-text source map from a [`LinkedRom`].
///
/// Each line has the form `<rom_offset_hex> <file_id> <line> <col>`.
/// Entries come from the `__src_<N>` marker labels the IR
/// codegen emits for every [`crate::ir::IrOp::SourceLoc`] — one
/// per lowered source statement — paired against their original
/// spans via the codegen's `source_locs` side table passed in as
/// `source_locs`.
///
/// `source` is the preprocessed source text. We translate each
/// span's byte offset into a `(line, col)` pair by scanning
/// through the source once. Files included via `include` share
/// the same file id in the preprocessed text, so a single scan
/// covers every entry.
///
/// The output is sorted by ROM offset so the file is diff-
/// friendly and so downstream tools can binary-search by address.
#[must_use]
pub fn render_source_map(
    linked: &LinkedRom,
    source_locs: &[(String, Span)],
    source: &str,
) -> String {
    let mut entries: Vec<(usize, u16, u32, u32)> = Vec::new();
    let base_cpu_addr: u16 = 0xC000;
    for (label, span) in source_locs {
        let Some(&cpu_addr) = linked.labels.get(label) else {
            continue;
        };
        if cpu_addr < base_cpu_addr {
            continue;
        }
        let rom_offset = linked.fixed_bank_file_offset + (cpu_addr - base_cpu_addr) as usize;
        let prg_offset = rom_offset.saturating_sub(16);
        let (line, col) = byte_offset_to_line_col(source, span.start as usize);
        entries.push((prg_offset, span.file_id, line, col));
    }
    entries.sort_unstable();

    let mut out = String::new();
    for (offset, file_id, line, col) in entries {
        let _ = writeln!(out, "{offset:04X} {file_id} {line} {col}");
    }
    out
}

/// Convert a byte offset into `source` into a 1-based
/// `(line, column)` pair. Used by the source-map emitter to
/// translate the compact byte-offset spans carried inside
/// [`crate::lexer::Span`] into human-readable positions. Offsets
/// past the end of the source clamp to the last line.
fn byte_offset_to_line_col(source: &str, offset: usize) -> (u32, u32) {
    let bytes = source.as_bytes();
    let limit = offset.min(bytes.len());
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for &b in &bytes[..limit] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_linked(labels: &[(&str, u16)]) -> LinkedRom {
        let mut map = HashMap::new();
        for (name, addr) in labels {
            map.insert((*name).to_string(), *addr);
        }
        LinkedRom {
            rom: Vec::new(),
            labels: map,
            fixed_bank_file_offset: 16,
        }
    }

    #[test]
    fn mlb_skips_internal_labels_and_keeps_entry_points() {
        let linked = make_linked(&[
            ("__reset", 0xC010),
            ("__nmi", 0xC020),
            ("__ir_fn_Main_frame", 0xC100),
            ("__ir_fn_helper", 0xC200),
            ("__ir_skip_17", 0xC108), // internal, should not appear
            ("__ir_main_loop", 0xC080),
        ]);
        let vars = vec![
            VarAllocation {
                name: "score".into(),
                address: 0x0010,
                size: 1,
            },
            VarAllocation {
                name: "enemies".into(),
                address: 0x0300,
                size: 4,
            },
        ];
        let out = render_mlb(&linked, &vars);
        assert!(out.contains("Main_frame"), "should strip __ir_fn_ prefix");
        assert!(out.contains("helper"));
        assert!(out.contains("main_loop"));
        assert!(out.contains("reset"));
        assert!(out.contains("nmi"));
        assert!(
            !out.contains("__ir_skip_17"),
            "internal skip labels should not leak into the .mlb file"
        );
        // Var entries use the `R:` prefix and the raw RAM address.
        assert!(out.contains("R:0010:score"));
        assert!(out.contains("R:0300:enemies"));
    }

    #[test]
    fn mlb_uses_prg_relative_offsets() {
        // A label at CPU $C010 should land at PRG offset 0x0010
        // — the fixed bank's first byte sits at ROM file offset
        // 16 (post-header) and the .mlb format strips that
        // header back off.
        let linked = make_linked(&[("__ir_fn_foo", 0xC010)]);
        let out = render_mlb(&linked, &[]);
        assert!(
            out.contains("P:0010:foo"),
            "PRG-relative offset should be 0x0010, got:\n{out}"
        );
    }

    #[test]
    fn source_map_resolves_spans_to_line_and_column() {
        let source = "line one\nsecond line\nthird\n";
        // Byte offset 9 is the start of "second", which is line
        // 2 column 1.
        let span = Span::new(0, 9, 15);
        let linked = make_linked(&[("__src_0", 0xC000)]);
        let out = render_source_map(&linked, &[("__src_0".to_string(), span)], source);
        // PRG offset 0 (fixed_bank_file_offset=16, then subtract
        // 16 for PRG-relative) file_id 0 line 2 col 1.
        assert_eq!(out.trim(), "0000 0 2 1", "got:\n{out}");
    }

    #[test]
    fn source_map_output_is_sorted_by_offset() {
        let source = "a\nb\nc\n";
        let linked = make_linked(&[("__src_0", 0xC020), ("__src_1", 0xC010)]);
        let out = render_source_map(
            &linked,
            &[
                ("__src_0".to_string(), Span::new(0, 0, 1)),
                ("__src_1".to_string(), Span::new(0, 2, 3)),
            ],
            source,
        );
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("0010"));
        assert!(lines[1].starts_with("0020"));
    }
}
