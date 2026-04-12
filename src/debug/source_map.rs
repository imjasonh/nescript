use std::collections::HashMap;
use std::fmt::Write;

use crate::analyzer::VarAllocation;
use crate::lexer::Span;

/// A mapping from ROM address to source location.
#[derive(Debug, Clone)]
pub struct SourceMapping {
    pub rom_address: u16,
    pub span: Span,
    pub label: Option<String>,
}

/// Source map for debug symbol output.
/// Maps ROM addresses back to `NEScript` source locations.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub mappings: Vec<SourceMapping>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, rom_address: u16, span: Span, label: Option<String>) {
        self.mappings.push(SourceMapping {
            rom_address,
            span,
            label,
        });
    }

    /// Sort mappings by ROM address.
    pub fn finalize(&mut self) {
        self.mappings.sort_by_key(|m| m.rom_address);
    }
}

/// Debug symbols for a compiled ROM.
/// Compatible with Mesen's .dbg format.
#[derive(Debug, Clone)]
pub struct DebugSymbols {
    pub source_map: SourceMap,
    pub symbols: HashMap<String, u16>,
    pub source_file: String,
}

impl DebugSymbols {
    pub fn new(source_file: &str) -> Self {
        Self {
            source_map: SourceMap::new(),
            symbols: HashMap::new(),
            source_file: source_file.to_string(),
        }
    }

    /// Add variable symbols from analysis.
    pub fn add_variables(&mut self, allocations: &[VarAllocation]) {
        for alloc in allocations {
            self.symbols.insert(alloc.name.clone(), alloc.address);
        }
    }

    /// Generate Mesen-compatible .mlb (label) file content.
    pub fn to_mesen_labels(&self) -> String {
        let mut output = String::new();
        for (name, &addr) in &self.symbols {
            // Mesen label format: P:ADDR:NAME (P = PRG ROM)
            let _ = writeln!(output, "P:{addr:04X}:{name}");
        }
        // Sort for deterministic output
        let mut lines: Vec<&str> = output.lines().collect();
        lines.sort_unstable();
        lines.join("\n") + "\n"
    }

    /// Generate a simple .sym (symbol table) file.
    pub fn to_sym_file(&self) -> String {
        let mut output = String::from("; NEScript debug symbols\n");
        let mut entries: Vec<_> = self.symbols.iter().collect();
        entries.sort_by_key(|(_, &addr)| addr);
        for (name, &addr) in &entries {
            let _ = writeln!(output, "{addr:04X} {name}");
        }
        output
    }
}
