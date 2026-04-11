use crate::lexer::Span;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    #[allow(dead_code)]
    Info,
}

/// Error codes organized by compiler phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    // E01xx: Lexer errors
    E0101, // unterminated string
    E0102, // invalid character
    E0103, // number literal overflow

    // E02xx: Type errors
    E0201, // type mismatch
    #[allow(dead_code)]
    E0202, // invalid cast
    E0203, // invalid operation for type

    // E03xx: Memory errors
    #[allow(dead_code)]
    E0301, // zero-page overflow

    // E04xx: Control flow errors
    E0401, // call depth exceeded
    E0402, // recursion detected
    #[allow(dead_code)]
    E0403, // unreachable state
    E0404, // transition to undefined state

    // E05xx: Declaration errors
    E0501, // duplicate declaration
    E0502, // undefined variable
    E0503, // undefined function
    E0504, // missing start declaration
    #[allow(dead_code)]
    E0505, // multiple start declarations

    // W01xx: Warnings
    #[allow(dead_code)]
    W0101, // expensive multiply/divide operation
    #[allow(dead_code)]
    W0102, // loop without break or wait_frame
    #[allow(dead_code)]
    W0103, // unused variable
    #[allow(dead_code)]
    W0104, // unreachable code
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Self::E0101 => "E0101",
            Self::E0102 => "E0102",
            Self::E0103 => "E0103",
            Self::E0201 => "E0201",
            Self::E0202 => "E0202",
            Self::E0203 => "E0203",
            Self::E0301 => "E0301",
            Self::E0401 => "E0401",
            Self::E0402 => "E0402",
            Self::E0403 => "E0403",
            Self::E0404 => "E0404",
            Self::E0501 => "E0501",
            Self::E0502 => "E0502",
            Self::E0503 => "E0503",
            Self::E0504 => "E0504",
            Self::E0505 => "E0505",
            Self::W0101 => "W0101",
            Self::W0102 => "W0102",
            Self::W0103 => "W0103",
            Self::W0104 => "W0104",
        };
        write!(f, "{code}")
    }
}

impl ErrorCode {
    pub fn level(self) -> Level {
        match self {
            Self::W0101 | Self::W0102 | Self::W0103 | Self::W0104 => Level::Warning,
            _ => Level::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
    pub labels: Vec<Label>,
    pub help: Option<String>,
    pub note: Option<String>,
}

impl Diagnostic {
    pub fn error(code: ErrorCode, message: impl Into<String>, span: Span) -> Self {
        Self {
            level: code.level(),
            code,
            message: message.into(),
            span,
            labels: Vec::new(),
            help: None,
            note: None,
        }
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    #[must_use]
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn is_error(&self) -> bool {
        self.level == Level::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Info => "info",
        };
        write!(f, "{level}[{}]: {}", self.code, self.message)
    }
}
