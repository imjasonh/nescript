use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub game: GameDecl,
    pub globals: Vec<VarDecl>,
    pub constants: Vec<ConstDecl>,
    pub functions: Vec<FunDecl>,
    pub states: Vec<StateDecl>,
    pub sprites: Vec<SpriteDecl>,
    pub palettes: Vec<PaletteDecl>,
    pub backgrounds: Vec<BackgroundDecl>,
    pub banks: Vec<BankDecl>,
    pub start_state: String,
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
}

impl std::fmt::Display for NesType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::U8 => write!(f, "u8"),
            Self::I8 => write!(f, "i8"),
            Self::U16 => write!(f, "u16"),
            Self::Bool => write!(f, "bool"),
            Self::Array(t, n) => write!(f, "{t}[{n}]"),
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
