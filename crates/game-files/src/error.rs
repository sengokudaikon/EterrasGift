use std::fmt;

/// Why a file could not be read as expected. Every failure is final for that file: nothing is guessed or skipped.
#[derive(Debug)]
pub enum Error {
    Io {
        path: String,
        source: std::io::Error,
    },
    /// The bytes are not what the format says they must be (truncated, out of bounds, wrong magic).
    Malformed(String),
    /// A valid file using something this reader does not handle (a compression, a format version, a node type).
    Unsupported(String),
    /// The file is readable but the game data in it is not where or what this reader expects.
    Data(String),
}

impl Error {
    pub(crate) fn malformed(message: impl Into<String>) -> Self {
        Self::Malformed(message.into())
    }
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }
    pub(crate) fn data(message: impl Into<String>) -> Self {
        Self::Data(message.into())
    }
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// Prefixes the message with where it happened, keeping the kind.
    pub(crate) fn context(self, place: impl fmt::Display) -> Self {
        match self {
            Self::Io { path, source } => Self::Io { path, source },
            Self::Malformed(m) => Self::Malformed(format!("{place}: {m}")),
            Self::Unsupported(m) => Self::Unsupported(format!("{place}: {m}")),
            Self::Data(m) => Self::Data(format!("{place}: {m}")),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{path}: {source}"),
            Self::Malformed(m) => write!(f, "malformed: {m}"),
            Self::Unsupported(m) => write!(f, "unsupported: {m}"),
            Self::Data(m) => write!(f, "unexpected game data: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
