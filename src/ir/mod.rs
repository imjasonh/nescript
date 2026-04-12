mod lowering;
#[cfg(test)]
mod tests;

pub use lowering::{lower, RAW_ASM_PREFIX};

use crate::lexer::Span;
use std::fmt;

/// A unique identifier for a variable across the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarId(pub u32);

/// A virtual register — unlimited supply, resolved during codegen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IrTemp(pub u32);

impl fmt::Display for IrTemp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

impl fmt::Display for VarId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// The top-level IR program.
#[derive(Debug, Clone)]
pub struct IrProgram {
    pub functions: Vec<IrFunction>,
    pub globals: Vec<IrGlobal>,
    pub rom_data: Vec<IrRomBlock>,
    /// Ordered list of state names (index = state dispatch number).
    pub states: Vec<String>,
    /// Name of the initial state when the ROM boots.
    pub start_state: String,
}

/// A global variable in the IR.
#[derive(Debug, Clone)]
pub struct IrGlobal {
    pub var_id: VarId,
    pub name: String,
    pub size: u16,
    pub init_value: Option<u16>,
}

/// A block of constant data to be placed in ROM.
#[derive(Debug, Clone)]
pub struct IrRomBlock {
    pub label: String,
    pub data: Vec<u8>,
}

/// An IR function (includes state handlers, user functions, etc.)
#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub blocks: Vec<IrBasicBlock>,
    pub locals: Vec<IrLocal>,
    pub param_count: usize,
    pub has_return: bool,
    pub source_span: Span,
}

/// A local variable within a function.
#[derive(Debug, Clone)]
pub struct IrLocal {
    pub var_id: VarId,
    pub name: String,
    pub size: u16,
}

/// A basic block — a straight-line sequence of ops ending with a terminator.
#[derive(Debug, Clone)]
pub struct IrBasicBlock {
    pub label: String,
    pub ops: Vec<IrOp>,
    pub terminator: IrTerminator,
}

/// An IR operation.
#[derive(Debug, Clone)]
pub enum IrOp {
    // Load/Store
    LoadImm(IrTemp, u8),
    LoadVar(IrTemp, VarId),
    StoreVar(VarId, IrTemp),

    // Arithmetic (8-bit)
    Add(IrTemp, IrTemp, IrTemp),
    Sub(IrTemp, IrTemp, IrTemp),
    Mul(IrTemp, IrTemp, IrTemp),
    And(IrTemp, IrTemp, IrTemp),
    Or(IrTemp, IrTemp, IrTemp),
    Xor(IrTemp, IrTemp, IrTemp),
    ShiftLeft(IrTemp, IrTemp, u8),
    ShiftRight(IrTemp, IrTemp, u8),
    Negate(IrTemp, IrTemp),
    Complement(IrTemp, IrTemp),

    // Comparison (sets a boolean temp)
    CmpEq(IrTemp, IrTemp, IrTemp),
    CmpNe(IrTemp, IrTemp, IrTemp),
    CmpLt(IrTemp, IrTemp, IrTemp),
    CmpGt(IrTemp, IrTemp, IrTemp),
    CmpLtEq(IrTemp, IrTemp, IrTemp),
    CmpGtEq(IrTemp, IrTemp, IrTemp),

    // Array access
    ArrayLoad(IrTemp, VarId, IrTemp),
    ArrayStore(VarId, IrTemp, IrTemp),

    // Function call
    Call(Option<IrTemp>, String, Vec<IrTemp>),

    // Hardware operations
    DrawSprite {
        sprite_name: String,
        x: IrTemp,
        y: IrTemp,
        frame: Option<IrTemp>,
    },
    /// Read a controller input byte into a temp.
    /// Second arg: 0 for player 1, 1 for player 2.
    ReadInput(IrTemp, u8),
    WaitFrame,
    Transition(String),
    /// Write PPU scroll registers (two writes to $2005: X then Y).
    Scroll(IrTemp, IrTemp),
    /// Debug: write a list of temps to the emulator debug port ($4800).
    /// Stripped in release mode by the codegen.
    DebugLog(Vec<IrTemp>),
    /// Debug: runtime assertion — if `cond` is zero, halt with debug marker.
    /// Stripped in release mode by the codegen.
    DebugAssert(IrTemp),
    /// Raw 6502 assembly text; parsed and emitted by the codegen.
    InlineAsm(String),

    // Source mapping
    SourceLoc(Span),
}

/// A basic block terminator.
#[derive(Debug, Clone)]
pub enum IrTerminator {
    /// Unconditional jump to a label.
    Jump(String),
    /// Conditional branch: if temp != 0 goto `true_label` else goto `false_label`.
    Branch(IrTemp, String, String),
    /// Return from function, optionally with a value.
    Return(Option<IrTemp>),
    /// Unreachable (after infinite loops, etc.)
    Unreachable,
}

impl IrProgram {
    /// Count total number of IR operations across all functions.
    pub fn op_count(&self) -> usize {
        self.functions
            .iter()
            .flat_map(|f| &f.blocks)
            .map(|b| b.ops.len())
            .sum()
    }

    /// Human-readable pretty-print of the entire program — used by
    /// the `--dump-ir` CLI flag and by debugging sessions.
    pub fn pretty(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        out.push_str("# IR Program\n");
        let _ = writeln!(out, "# start_state = {}", self.start_state);
        let _ = writeln!(out, "# states = {:?}", self.states);
        if !self.globals.is_empty() {
            out.push_str("\n# Globals\n");
            for g in &self.globals {
                let _ = writeln!(
                    out,
                    "  {} {} (size={}) = {:?}",
                    g.var_id, g.name, g.size, g.init_value
                );
            }
        }
        for func in &self.functions {
            let _ = writeln!(
                out,
                "\nfn {}({} params, has_return={}):",
                func.name, func.param_count, func.has_return
            );
            for block in &func.blocks {
                let _ = writeln!(out, "  {}:", block.label);
                for op in &block.ops {
                    let _ = writeln!(out, "    {op:?}");
                }
                let _ = writeln!(out, "    -> {:?}", block.terminator);
            }
        }
        out
    }
}

impl IrFunction {
    /// Count total number of IR operations in this function.
    pub fn op_count(&self) -> usize {
        self.blocks.iter().map(|b| b.ops.len()).sum()
    }
}
