use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub game: GameDecl,
    pub globals: Vec<VarDecl>,
    pub constants: Vec<ConstDecl>,
    pub enums: Vec<EnumDecl>,
    pub structs: Vec<StructDecl>,
    pub functions: Vec<FunDecl>,
    pub states: Vec<StateDecl>,
    pub sprites: Vec<SpriteDecl>,
    pub palettes: Vec<PaletteDecl>,
    pub backgrounds: Vec<BackgroundDecl>,
    pub banks: Vec<BankDecl>,
    pub start_state: String,
    pub span: Span,
}

/// `enum Name { V1, V2, V3 }` — variants become u8 constants with
/// values equal to their declaration order (0, 1, 2, ...). Variant
/// names are global: they're flattened into the constants table so
/// they can be referenced directly without namespacing.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<(String, Span)>,
    pub span: Span,
}

/// `struct Name { field1: u8, field2: u8 }` — composite type with a
/// known layout. Fields are stored contiguously in memory in
/// declaration order (no padding). Only primitive-sized fields (u8,
/// i8, bool) are supported in the v1 layout.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<StructField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub field_type: NesType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SpriteDecl {
    pub name: String,
    pub chr_source: AssetSource,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PaletteDecl {
    pub name: String,
    pub colors: Vec<u8>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BackgroundDecl {
    pub name: String,
    pub chr_source: AssetSource,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AssetSource {
    Chr(String),
    Binary(String),
    Inline(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct GameDecl {
    pub name: String,
    pub mapper: Mapper,
    pub mirroring: Mirroring,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapper {
    NROM,
    MMC1,
    UxROM,
    MMC3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct BankDecl {
    pub name: String,
    pub bank_type: BankType,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankType {
    Prg,
    Chr,
}

#[derive(Debug, Clone)]
pub struct StateDecl {
    pub name: String,
    pub locals: Vec<VarDecl>,
    pub on_enter: Option<Block>,
    pub on_exit: Option<Block>,
    pub on_frame: Option<Block>,
    pub on_scanline: Vec<(u8, Block)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<NesType>,
    pub body: Block,
    pub is_inline: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub param_type: NesType,
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub name: String,
    pub var_type: NesType,
    pub init: Option<Expr>,
    pub placement: Placement,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub const_type: NesType,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Fast,
    Slow,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NesType {
    U8,
    I8,
    U16,
    Bool,
    Array(Box<NesType>, u16),
    /// A user-declared struct, identified by its name. The analyzer
    /// looks up field layouts in the `StructDecl` table.
    Struct(String),
}

impl std::fmt::Display for NesType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::U8 => write!(f, "u8"),
            Self::I8 => write!(f, "i8"),
            Self::U16 => write!(f, "u16"),
            Self::Bool => write!(f, "bool"),
            Self::Array(t, n) => write!(f, "{t}[{n}]"),
            Self::Struct(name) => write!(f, "{name}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expr {
    IntLiteral(u16, Span),
    BoolLiteral(bool, Span),
    Ident(String, Span),
    ArrayIndex(String, Box<Expr>, Span),
    /// Field access on a struct variable: `pos.x`.
    FieldAccess(String, String, Span),
    BinaryOp(Box<Expr>, BinOp, Box<Expr>, Span),
    UnaryOp(UnaryOp, Box<Expr>, Span),
    Call(String, Vec<Expr>, Span),
    ButtonRead(Option<Player>, String, Span),
    ArrayLiteral(Vec<Expr>, Span),
    Cast(Box<Expr>, NesType, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Self::IntLiteral(_, s)
            | Self::BoolLiteral(_, s)
            | Self::Ident(_, s)
            | Self::ArrayIndex(_, _, s)
            | Self::FieldAccess(_, _, s)
            | Self::BinaryOp(_, _, _, s)
            | Self::UnaryOp(_, _, s)
            | Self::Call(_, _, s)
            | Self::ButtonRead(_, _, s)
            | Self::ArrayLiteral(_, s)
            | Self::Cast(_, _, s) => *s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    P1,
    P2,
}

#[derive(Debug, Clone)]
pub enum Statement {
    VarDecl(VarDecl),
    Assign(LValue, AssignOp, Expr, Span),
    If(Expr, Block, Vec<(Expr, Block)>, Option<Block>, Span),
    While(Expr, Block, Span),
    Loop(Block, Span),
    /// `for NAME in START..END { BODY }` — half-open range.
    /// Lowers to an index variable + while loop in IR.
    For {
        var: String,
        start: Expr,
        end: Expr,
        body: Block,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Return(Option<Expr>, Span),
    Draw(DrawStmt),
    Transition(String, Span),
    WaitFrame(Span),
    Call(String, Vec<Expr>, Span),
    LoadBackground(String, Span),
    SetPalette(String, Span),
    Scroll(Expr, Expr, Span),
    /// debug.log(expr, ...) — writes values to the emulator debug port.
    /// Stripped in release mode.
    DebugLog(Vec<Expr>, Span),
    /// debug.assert(cond) — runtime check, halts on failure.
    /// Stripped in release mode.
    DebugAssert(Expr, Span),
    /// Inline 6502 assembly captured verbatim. The body is parsed by
    /// the codegen stage using `asm::parse_inline`.
    InlineAsm(String, Span),
    /// Audio: `play SfxName` — trigger a one-shot sound effect.
    /// Currently a no-op at codegen time; no audio driver exists.
    Play(String, Span),
    /// Audio: `start_music TrackName` — begin playing background music.
    /// Currently a no-op at codegen time.
    StartMusic(String, Span),
    /// Audio: `stop_music` — stop any currently-playing music.
    /// Currently a no-op at codegen time.
    StopMusic(Span),
}

impl Statement {
    pub fn span(&self) -> Span {
        match self {
            Self::VarDecl(v) => v.span,
            Self::Draw(d) => d.span,
            Self::For { span, .. } => *span,
            Self::Assign(_, _, _, s)
            | Self::If(_, _, _, _, s)
            | Self::While(_, _, s)
            | Self::Loop(_, s)
            | Self::Break(s)
            | Self::Continue(s)
            | Self::Return(_, s)
            | Self::Transition(_, s)
            | Self::WaitFrame(s)
            | Self::Call(_, _, s)
            | Self::LoadBackground(_, s)
            | Self::SetPalette(_, s)
            | Self::Scroll(_, _, s)
            | Self::DebugLog(_, s)
            | Self::DebugAssert(_, s)
            | Self::InlineAsm(_, s)
            | Self::Play(_, s)
            | Self::StartMusic(_, s)
            | Self::StopMusic(s) => *s,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DrawStmt {
    pub sprite_name: String,
    pub x: Expr,
    pub y: Expr,
    pub frame: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum LValue {
    Var(String),
    ArrayIndex(String, Box<Expr>),
    /// Struct field: `pos.x = 5`. First string is the struct variable
    /// name, second is the field name.
    Field(String, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    PlusAssign,
    MinusAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    ShiftLeftAssign,
    ShiftRightAssign,
}
