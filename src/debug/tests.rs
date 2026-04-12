use super::*;
use crate::lexer::Span;

#[test]
fn source_map_add_and_finalize() {
    let mut sm = SourceMap::new();
    sm.add(0xC010, Span::new(0, 10, 20), Some("main_loop".into()));
    sm.add(0xC000, Span::new(0, 0, 5), Some("reset".into()));
    sm.finalize();

    // Should be sorted by ROM address
    assert_eq!(sm.mappings[0].rom_address, 0xC000);
    assert_eq!(sm.mappings[1].rom_address, 0xC010);
}

#[test]
fn debug_symbols_new() {
    let ds = DebugSymbols::new("test.ne");
    assert_eq!(ds.source_file, "test.ne");
    assert!(ds.symbols.is_empty());
}

#[test]
fn debug_symbols_add_variables() {
    use crate::analyzer::VarAllocation;

    let mut ds = DebugSymbols::new("test.ne");
    ds.add_variables(&[
        VarAllocation {
            name: "px".into(),
            address: 0x10,
            size: 1,
        },
        VarAllocation {
            name: "py".into(),
            address: 0x11,
            size: 1,
        },
    ]);
    assert_eq!(ds.symbols["px"], 0x10);
    assert_eq!(ds.symbols["py"], 0x11);
}

#[test]
fn mesen_labels_format() {
    let mut ds = DebugSymbols::new("test.ne");
    ds.symbols.insert("player_x".into(), 0x0010);
    ds.symbols.insert("score".into(), 0x0012);

    let labels = ds.to_mesen_labels();
    assert!(labels.contains("P:0010:player_x"));
    assert!(labels.contains("P:0012:score"));
}

#[test]
fn sym_file_format() {
    let mut ds = DebugSymbols::new("test.ne");
    ds.symbols.insert("px".into(), 0x10);
    ds.symbols.insert("py".into(), 0x11);

    let sym = ds.to_sym_file();
    assert!(sym.contains("0010 px"));
    assert!(sym.contains("0011 py"));
    // Should have header comment
    assert!(sym.starts_with("; NEScript"));
}
