//! Audio asset resolver.
//!
//! Compiles user-declared `sfx` and `music` blocks into the flat byte
//! tables consumed by the runtime audio driver, and provides builtin
//! fallback effects and tracks for unrecognized names (so legacy
//! programs that `play coin` / `start_music theme` without declaring
//! them still make sound).
//!
//! ## SFX representation
//!
//! Each [`SfxData`] is:
//!
//! - A few *compile-time constants* (`period_lo`, `period_hi`) that
//!   the IR codegen emits as immediates in the `play` instruction
//!   sequence to trigger a new pulse-1 note.
//! - A raw *envelope byte stream* stored in PRG ROM under the label
//!   `__sfx_<name>`, consumed one byte per NMI by the runtime audio
//!   tick. Each byte is a complete `$4000` write (duty<<6 | 0x30 |
//!   volume). A trailing zero byte is the sentinel: the tick sees
//!   it, mutes pulse 1, and stops.
//!
//! ## Music representation
//!
//! Each [`MusicData`] is:
//!
//! - A single compile-time `header` byte cached in ZP by
//!   `start_music` and used by the tick to build `$4004` envelope
//!   writes on every note change. Bit layout:
//!   - bit 0: loop flag
//!   - bits 2-5: volume (0-15)
//!   - bits 6-7: duty (0-3)
//! - A raw `(pitch, dur)` *note stream* stored in PRG ROM under the
//!   label `__music_<name>`, terminated by `(0xFF, 0xFF)`. Pitch 0 is
//!   a rest; pitches 1-60 are indices into the period table.

use crate::parser::ast::{MusicDecl, MusicNote, Program, SfxDecl};

/// Compiled sfx data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfxData {
    pub name: String,
    /// Pulse-1 period low byte, written to `$4002` by `play` as an
    /// immediate. Determines the tone of the effect.
    pub period_lo: u8,
    /// Pulse-1 length counter + period high byte, written to
    /// `$4003`. Triggers a new note on write.
    pub period_hi: u8,
    /// Per-frame `$4000` envelope bytes terminated by `0x00`. Linked
    /// into PRG ROM as a labelled data block.
    pub envelope: Vec<u8>,
}

/// Compiled music data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicData {
    pub name: String,
    /// State byte cached by `start_music` into `ZP_MUSIC_STATE`. Bit
    /// 1 is OR'd in at runtime to mark the track active. Encodes
    /// duty (bits 6-7), volume (bits 2-5), and loop flag (bit 0).
    pub header: u8,
    /// Raw `(pitch, duration)` note stream terminated by
    /// `(0xFF, 0xFF)`. Linked into PRG ROM as a labelled data block.
    pub stream: Vec<u8>,
}

impl SfxData {
    /// ROM label for the in-PRG envelope blob. The IR codegen
    /// emits `LDA #<label` / `LDA #>label` pairs to load the
    /// pointer into `ZP_SFX_PTR_LO/HI` on `play`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("__sfx_{}", sanitize_label(&self.name))
    }
}

impl MusicData {
    /// ROM label for the in-PRG note-stream blob.
    #[must_use]
    pub fn label(&self) -> String {
        format!("__music_{}", sanitize_label(&self.name))
    }
}

/// Turn an audio asset name into a label-safe identifier. The
/// public API already restricts names to valid identifiers via the
/// parser, so this only has to protect against nonstandard input
/// from builtins (which currently use lowercase words).
fn sanitize_label(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Per-frame max (user-declared sfx length cap).
///
/// The driver walks envelope bytes one per NMI — a 60-frame sfx
/// lasts one second on NTSC. Anything much longer than that starts
/// overlapping with the next trigger and sounds muddy; cap at 120
/// (2 seconds) as a sanity check rather than a hard ROM constraint.
pub const SFX_MAX_FRAMES: usize = 120;

/// Per-track max notes (user-declared music length cap). The music
/// blob is 2 bytes per note plus a header and a 2-byte sentinel, so
/// 256 notes costs 515 ROM bytes — plenty for typical game loops.
pub const MUSIC_MAX_NOTES: usize = 256;

/// Resolve all user-declared `sfx` blocks in a program into compiled
/// byte blobs. Also appends builtin sfx data for any name referenced
/// in a `play` statement that isn't user-declared — this keeps legacy
/// programs working without explicit `sfx` declarations.
///
/// Returns an error if two user declarations share the same name, or
/// if any declaration exceeds the sfx length cap.
pub fn resolve_sfx(program: &Program) -> Result<Vec<SfxData>, String> {
    let mut out: Vec<SfxData> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1. User-declared sfx come first. Name collisions are an error.
    for decl in &program.sfx {
        if !seen.insert(decl.name.clone()) {
            return Err(format!("duplicate sfx declaration: '{}'", decl.name));
        }
        if decl.pitch.len() > SFX_MAX_FRAMES {
            return Err(format!(
                "sfx '{}' has {} frames, max is {}",
                decl.name,
                decl.pitch.len(),
                SFX_MAX_FRAMES
            ));
        }
        out.push(compile_sfx(decl));
    }

    // 2. Append builtin sfx for any referenced-but-undeclared names.
    // Walk every `play name` statement in the program and, for each
    // unfamiliar name that matches a builtin, synthesize a builtin
    // decl and compile it. We only emit each builtin once.
    let referenced = referenced_sfx_names(program);
    for name in &referenced {
        if seen.contains(name) {
            continue;
        }
        if let Some(decl) = builtin_sfx(name) {
            seen.insert(name.clone());
            out.push(compile_sfx(&decl));
        }
    }

    Ok(out)
}

/// Resolve all user-declared `music` blocks. Same shape as
/// [`resolve_sfx`] — user decls first, then builtins for any
/// referenced-but-undeclared names.
pub fn resolve_music(program: &Program) -> Result<Vec<MusicData>, String> {
    let mut out: Vec<MusicData> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for decl in &program.music {
        if !seen.insert(decl.name.clone()) {
            return Err(format!("duplicate music declaration: '{}'", decl.name));
        }
        if decl.notes.len() > MUSIC_MAX_NOTES {
            return Err(format!(
                "music '{}' has {} notes, max is {}",
                decl.name,
                decl.notes.len(),
                MUSIC_MAX_NOTES
            ));
        }
        out.push(compile_music(decl));
    }

    let referenced = referenced_music_names(program);
    for name in &referenced {
        if seen.contains(name) {
            continue;
        }
        if let Some(decl) = builtin_music(name) {
            seen.insert(name.clone());
            out.push(compile_music(&decl));
        }
    }

    Ok(out)
}

/// Compile one sfx declaration into its `SfxData`.
///
/// The compile-time constants (period) are derived from the *first*
/// pitch value in the array — v1 of the driver holds a fixed period
/// for the whole envelope so the pulse channel doesn't retrigger.
/// Pitch variation across frames is ignored in this version; a
/// richer tracker-style format could interleave period and volume
/// updates, but the simple envelope is plenty expressive for the
/// classic set of game sounds.
fn compile_sfx(decl: &SfxDecl) -> SfxData {
    let period_lo = decl.pitch.first().copied().unwrap_or(0);
    // length_hi: length counter load index 0 (254 frames), period hi = 0.
    // Bit 3 of $4003 = length counter enable; bits 0-2 = period high.
    let period_hi: u8 = 0x08;
    let mut envelope = Vec::with_capacity(decl.volume.len() + 1);
    for &vol in &decl.volume {
        let duty = decl.duty & 0x03;
        // $4000 format: DD LC VVVV. We always set L (length-halt)
        // and C (constant volume) so the envelope value is exactly
        // the user's volume number without APU envelope decay.
        let env = (duty << 6) | 0x30 | (vol & 0x0F);
        envelope.push(env);
    }
    // Zero sentinel — the audio tick sees this, mutes pulse 1, and
    // clears the sfx counter so subsequent NMIs don't keep walking.
    envelope.push(0x00);
    SfxData {
        name: decl.name.clone(),
        period_lo,
        period_hi,
        envelope,
    }
}

/// Compile one music declaration into its `MusicData`.
fn compile_music(decl: &MusicDecl) -> MusicData {
    let duty = decl.duty & 0x03;
    let volume = decl.volume & 0x0F;
    let header = (duty << 6) | (volume << 2) | u8::from(decl.loops);
    let mut stream = Vec::with_capacity(decl.notes.len() * 2 + 2);
    for note in &decl.notes {
        stream.push(note.pitch);
        stream.push(note.duration);
    }
    // End-of-track sentinel. The driver loops or mutes based on the
    // state header's loop bit (ORed in by `start_music`).
    stream.push(0xFF);
    stream.push(0xFF);
    MusicData {
        name: decl.name.clone(),
        header,
        stream,
    }
}

/// Collect every sfx name referenced by a `play NAME` statement
/// anywhere in the program.
fn referenced_sfx_names(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for state in &program.states {
        for block in state
            .on_enter
            .iter()
            .chain(state.on_exit.iter())
            .chain(state.on_frame.iter())
            .chain(state.on_scanline.iter().map(|(_, b)| b))
        {
            walk_for_play(block, &mut out);
        }
    }
    for func in &program.functions {
        walk_for_play(&func.body, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Collect every music name referenced by a `start_music NAME`
/// statement anywhere in the program.
fn referenced_music_names(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for state in &program.states {
        for block in state
            .on_enter
            .iter()
            .chain(state.on_exit.iter())
            .chain(state.on_frame.iter())
            .chain(state.on_scanline.iter().map(|(_, b)| b))
        {
            walk_for_music(block, &mut out);
        }
    }
    for func in &program.functions {
        walk_for_music(&func.body, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk_for_play(block: &crate::parser::ast::Block, out: &mut Vec<String>) {
    use crate::parser::ast::Statement;
    for stmt in &block.statements {
        match stmt {
            Statement::Play(name, _) => out.push(name.clone()),
            Statement::If(_, then_b, elifs, else_b, _) => {
                walk_for_play(then_b, out);
                for (_, b) in elifs {
                    walk_for_play(b, out);
                }
                if let Some(b) = else_b {
                    walk_for_play(b, out);
                }
            }
            Statement::While(_, b, _) => walk_for_play(b, out),
            Statement::Loop(b, _) => walk_for_play(b, out),
            Statement::For { body, .. } => walk_for_play(body, out),
            _ => {}
        }
    }
}

fn walk_for_music(block: &crate::parser::ast::Block, out: &mut Vec<String>) {
    use crate::parser::ast::Statement;
    for stmt in &block.statements {
        match stmt {
            Statement::StartMusic(name, _) => out.push(name.clone()),
            Statement::If(_, then_b, elifs, else_b, _) => {
                walk_for_music(then_b, out);
                for (_, b) in elifs {
                    walk_for_music(b, out);
                }
                if let Some(b) = else_b {
                    walk_for_music(b, out);
                }
            }
            Statement::While(_, b, _) => walk_for_music(b, out),
            Statement::Loop(b, _) => walk_for_music(b, out),
            Statement::For { body, .. } => walk_for_music(body, out),
            _ => {}
        }
    }
}

/// Return a builtin sfx declaration matching `name`, or `None` if the
/// name isn't a recognized builtin. Builtins cover the six classic
/// game-audio cliches: coin, jump, hit, click, cancel, shoot. Names
/// are matched case-insensitively with a few common aliases.
#[must_use]
pub fn builtin_sfx(name: &str) -> Option<SfxDecl> {
    use crate::lexer::Span;
    let lower = name.to_ascii_lowercase();
    let (duty, pitch_base, volume) = match lower.as_str() {
        // High, short, ascending blip — classic pickup chirp.
        "coin" | "pickup" | "collect" => (2u8, 0x50u8, vec![15, 14, 13, 11, 9, 7, 4, 2]),
        // Quick descending arc — jump ack.
        "jump" | "hop" => (2, 0x80, vec![13, 13, 12, 11, 9, 7, 5, 3]),
        // Low short blast — hit/damage/explode.
        "hit" | "damage" | "explode" => (1, 0xA0, vec![15, 14, 12, 10, 8, 6, 4, 2, 1]),
        // Sharp high beep — menu click/confirm.
        "click" | "select" | "confirm" => (2, 0x40, vec![12, 10, 6, 2]),
        // Low longer tone — cancel/back/error.
        "cancel" | "back" | "error" => (2, 0xB0, vec![14, 13, 12, 11, 10, 9, 8, 7, 6, 4]),
        // Very high, short — laser shoot.
        "shoot" | "laser" | "fire" => (3, 0x30, vec![15, 12, 9, 6, 3]),
        // Short low thud — footstep.
        "step" | "footstep" => (0, 0xC0, vec![10, 6, 2]),
        _ => return None,
    };
    // Pitch array is constant (one byte, since v1 format latches
    // the period once). Use pitch_base as the single entry.
    let frames = volume.len();
    Some(SfxDecl {
        name: name.to_string(),
        duty,
        pitch: vec![pitch_base; frames],
        volume,
        span: Span::dummy(),
    })
}

/// Return a builtin music declaration matching `name`, or `None`.
/// Builtins are short single-channel loops played on pulse 2.
#[must_use]
pub fn builtin_music(name: &str) -> Option<MusicDecl> {
    use crate::lexer::Span;
    // Note indexes reference the period table in
    // `runtime::gen_period_table`: 1 = C1, 13 = C2, 25 = C3, 37 = C4.
    // Middle C = 37.
    const REST: u8 = 0;
    const C4: u8 = 37;
    const D4: u8 = 39;
    const E4: u8 = 41;
    const F4: u8 = 42;
    const G4: u8 = 44;
    const A4: u8 = 46;
    const B4: u8 = 48;
    const C5: u8 = 49;
    let lower = name.to_ascii_lowercase();
    let notes: Vec<MusicNote> = match lower.as_str() {
        // Cheerful major arpeggio — default theme.
        "title" | "theme" | "main" => [
            (C4, 12),
            (E4, 12),
            (G4, 12),
            (C5, 12),
            (G4, 12),
            (E4, 12),
            (C4, 12),
            (REST, 12),
        ]
        .iter()
        .map(|&(p, d)| MusicNote {
            pitch: p,
            duration: d,
        })
        .collect(),
        // Fast driving pulse — battle/boss.
        "battle" | "boss" => [
            (A4, 8),
            (C5, 8),
            (E4, 8),
            (A4, 8),
            (G4, 8),
            (B4, 8),
            (D4, 8),
            (G4, 8),
        ]
        .iter()
        .map(|&(p, d)| MusicNote {
            pitch: p,
            duration: d,
        })
        .collect(),
        // Fanfare — short ascending burst.
        "win" | "victory" | "fanfare" => [(C4, 10), (E4, 10), (G4, 10), (C5, 20), (REST, 10)]
            .iter()
            .map(|&(p, d)| MusicNote {
                pitch: p,
                duration: d,
            })
            .collect(),
        // Gloomy descending — game over.
        "gameover" | "lose" | "fail" => {
            [(C4, 20), (B4, 20), (A4, 20), (G4, 20), (F4, 30), (REST, 20)]
                .iter()
                .map(|&(p, d)| MusicNote {
                    pitch: p,
                    duration: d,
                })
                .collect()
        }
        _ => return None,
    };
    Some(MusicDecl {
        name: name.to_string(),
        duty: 2,
        volume: 10,
        loops: !matches!(lower.as_str(), "win" | "victory" | "fanfare"),
        notes,
        span: Span::dummy(),
    })
}

/// Return true if `name` matches a builtin sfx entry.
#[must_use]
pub fn is_builtin_sfx(name: &str) -> bool {
    builtin_sfx(name).is_some()
}

/// Return true if `name` matches a builtin music entry.
#[must_use]
pub fn is_builtin_music(name: &str) -> bool {
    builtin_music(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Span;
    use crate::parser::ast::*;

    fn empty_program() -> Program {
        Program {
            game: GameDecl {
                name: "T".to_string(),
                mapper: Mapper::NROM,
                mirroring: Mirroring::Horizontal,
                span: Span::dummy(),
            },
            globals: Vec::new(),
            constants: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            states: Vec::new(),
            sprites: Vec::new(),
            palettes: Vec::new(),
            backgrounds: Vec::new(),
            sfx: Vec::new(),
            music: Vec::new(),
            banks: Vec::new(),
            start_state: "Main".to_string(),
            span: Span::dummy(),
        }
    }

    #[test]
    fn compile_sfx_splits_trigger_constants_from_envelope() {
        let decl = SfxDecl {
            name: "Test".to_string(),
            duty: 2,
            pitch: vec![0x50, 0x50, 0x50, 0x50],
            volume: vec![15, 10, 5, 0],
            span: Span::dummy(),
        };
        let data = compile_sfx(&decl);
        // Compile-time constants land in fields, not bytes.
        assert_eq!(data.period_lo, 0x50);
        assert_eq!(data.period_hi, 0x08);
        // Envelope = 4 frames + sentinel.
        assert_eq!(data.envelope.len(), 5);
        // First envelope byte: duty 2 << 6 | 0x30 | 15 = 0xBF.
        assert_eq!(data.envelope[0], 0xBF);
        // Second: 0x80 | 0x30 | 10 = 0xBA.
        assert_eq!(data.envelope[1], 0xBA);
        // Last byte is the mute sentinel.
        assert_eq!(*data.envelope.last().unwrap(), 0x00);
    }

    #[test]
    fn compile_sfx_duty_0_clears_top_bits() {
        let decl = SfxDecl {
            name: "Soft".to_string(),
            duty: 0,
            pitch: vec![0x20],
            volume: vec![8],
            span: Span::dummy(),
        };
        let data = compile_sfx(&decl);
        // env = 0 << 6 | 0x30 | 8 = 0x38
        assert_eq!(data.envelope[0], 0x38);
    }

    #[test]
    fn compile_music_header_encodes_loop_duty_volume() {
        let decl = MusicDecl {
            name: "Loop".to_string(),
            duty: 2,
            volume: 10,
            loops: true,
            notes: vec![MusicNote {
                pitch: 37,
                duration: 8,
            }],
            span: Span::dummy(),
        };
        let data = compile_music(&decl);
        // header = (2<<6) | (10<<2) | 1 = 0xA9
        let expected_header: u8 = (2 << 6) | (10 << 2) | 1;
        assert_eq!(data.header, expected_header);
        // Stream = (37, 8), (0xFF, 0xFF).
        assert_eq!(data.stream, vec![37, 8, 0xFF, 0xFF]);
    }

    #[test]
    fn compile_music_non_looping_clears_header_bit() {
        let decl = MusicDecl {
            name: "Once".to_string(),
            duty: 0,
            volume: 0,
            loops: false,
            notes: vec![MusicNote {
                pitch: 1,
                duration: 1,
            }],
            span: Span::dummy(),
        };
        let data = compile_music(&decl);
        assert_eq!(data.header & 0x01, 0, "loop bit must be clear");
    }

    #[test]
    fn sfx_label_is_deterministic_per_name() {
        let decl = SfxDecl {
            name: "Pickup".to_string(),
            duty: 2,
            pitch: vec![0x50],
            volume: vec![8],
            span: Span::dummy(),
        };
        let data = compile_sfx(&decl);
        assert_eq!(data.label(), "__sfx_Pickup");
    }

    #[test]
    fn music_label_sanitizes_special_chars() {
        let data = MusicData {
            name: "Title Screen".to_string(),
            header: 0,
            stream: vec![0xFF, 0xFF],
        };
        assert_eq!(data.label(), "__music_Title_Screen");
    }

    #[test]
    fn resolve_sfx_includes_user_decls() {
        let mut prog = empty_program();
        prog.sfx.push(SfxDecl {
            name: "Zap".to_string(),
            duty: 2,
            pitch: vec![0x40, 0x40],
            volume: vec![15, 8],
            span: Span::dummy(),
        });
        let resolved = resolve_sfx(&prog).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "Zap");
        // No play statements, so no builtins pulled in.
        assert!(!resolved.iter().any(|s| s.name == "coin"));
    }

    #[test]
    fn resolve_sfx_appends_builtin_when_referenced() {
        let mut prog = empty_program();
        // Simulate `on frame { play coin }` as a direct AST build.
        prog.states.push(StateDecl {
            name: "Main".to_string(),
            locals: Vec::new(),
            on_enter: None,
            on_exit: None,
            on_frame: Some(Block {
                statements: vec![Statement::Play("coin".to_string(), Span::dummy())],
                span: Span::dummy(),
            }),
            on_scanline: Vec::new(),
            span: Span::dummy(),
        });
        let resolved = resolve_sfx(&prog).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "coin");
    }

    #[test]
    fn resolve_sfx_user_decl_shadows_builtin() {
        // A user's `sfx coin { ... }` should take priority over the
        // builtin coin effect — otherwise `sfx coin` would be
        // confusing (users expect their definition to win).
        let mut prog = empty_program();
        prog.sfx.push(SfxDecl {
            name: "coin".to_string(),
            duty: 0,
            pitch: vec![0xAA],
            volume: vec![1],
            span: Span::dummy(),
        });
        prog.states.push(StateDecl {
            name: "Main".to_string(),
            locals: Vec::new(),
            on_enter: None,
            on_exit: None,
            on_frame: Some(Block {
                statements: vec![Statement::Play("coin".to_string(), Span::dummy())],
                span: Span::dummy(),
            }),
            on_scanline: Vec::new(),
            span: Span::dummy(),
        });
        let resolved = resolve_sfx(&prog).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].period_lo, 0xAA, "user period should win");
    }

    #[test]
    fn resolve_sfx_rejects_duplicates() {
        let mut prog = empty_program();
        prog.sfx.push(SfxDecl {
            name: "Boom".to_string(),
            duty: 2,
            pitch: vec![0x40],
            volume: vec![8],
            span: Span::dummy(),
        });
        prog.sfx.push(SfxDecl {
            name: "Boom".to_string(),
            duty: 2,
            pitch: vec![0x40],
            volume: vec![8],
            span: Span::dummy(),
        });
        assert!(resolve_sfx(&prog).is_err());
    }

    #[test]
    fn resolve_sfx_rejects_oversize() {
        let mut prog = empty_program();
        prog.sfx.push(SfxDecl {
            name: "Long".to_string(),
            duty: 2,
            pitch: vec![0; SFX_MAX_FRAMES + 1],
            volume: vec![8; SFX_MAX_FRAMES + 1],
            span: Span::dummy(),
        });
        assert!(resolve_sfx(&prog).is_err());
    }

    #[test]
    fn resolve_music_includes_user_decls() {
        let mut prog = empty_program();
        prog.music.push(MusicDecl {
            name: "Theme".to_string(),
            duty: 2,
            volume: 10,
            loops: true,
            notes: vec![MusicNote {
                pitch: 37,
                duration: 8,
            }],
            span: Span::dummy(),
        });
        let resolved = resolve_music(&prog).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "Theme");
    }

    #[test]
    fn resolve_music_appends_builtin_when_referenced() {
        let mut prog = empty_program();
        prog.states.push(StateDecl {
            name: "Main".to_string(),
            locals: Vec::new(),
            on_enter: None,
            on_exit: None,
            on_frame: Some(Block {
                statements: vec![Statement::StartMusic("theme".to_string(), Span::dummy())],
                span: Span::dummy(),
            }),
            on_scanline: Vec::new(),
            span: Span::dummy(),
        });
        let resolved = resolve_music(&prog).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "theme");
    }

    #[test]
    fn builtin_sfx_aliases_work() {
        assert!(builtin_sfx("coin").is_some());
        assert!(builtin_sfx("COIN").is_some());
        assert!(builtin_sfx("Pickup").is_some());
        assert!(builtin_sfx("unknown_nonsense").is_none());
    }

    #[test]
    fn builtin_music_aliases_work() {
        assert!(builtin_music("theme").is_some());
        assert!(builtin_music("BATTLE").is_some());
        assert!(builtin_music("Victory").is_some());
        assert!(builtin_music("not_a_track").is_none());
    }

    #[test]
    fn is_builtin_helpers_match_option_result() {
        for name in ["coin", "jump", "hit", "click", "cancel", "shoot", "step"] {
            assert!(is_builtin_sfx(name), "builtin sfx '{name}'");
        }
        for name in ["theme", "battle", "victory", "gameover"] {
            assert!(is_builtin_music(name), "builtin music '{name}'");
        }
        assert!(!is_builtin_sfx("totally_made_up"));
        assert!(!is_builtin_music("also_made_up"));
    }

    #[test]
    fn walk_for_play_finds_nested_references() {
        // `play` inside an `if` inside a `while` should still be
        // collected — the asset resolver needs to see every
        // referenced name so it can link in the right blob.
        let mut prog = empty_program();
        prog.states.push(StateDecl {
            name: "Main".to_string(),
            locals: Vec::new(),
            on_enter: None,
            on_exit: None,
            on_frame: Some(Block {
                statements: vec![Statement::While(
                    Expr::BoolLiteral(true, Span::dummy()),
                    Block {
                        statements: vec![Statement::If(
                            Expr::BoolLiteral(true, Span::dummy()),
                            Block {
                                statements: vec![Statement::Play(
                                    "jump".to_string(),
                                    Span::dummy(),
                                )],
                                span: Span::dummy(),
                            },
                            Vec::new(),
                            None,
                            Span::dummy(),
                        )],
                        span: Span::dummy(),
                    },
                    Span::dummy(),
                )],
                span: Span::dummy(),
            }),
            on_scanline: Vec::new(),
            span: Span::dummy(),
        });
        let names = referenced_sfx_names(&prog);
        assert_eq!(names, vec!["jump".to_string()]);
    }
}
