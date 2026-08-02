use std::io;

use thiserror::Error;

pub const EX_USAGE: i32 = 64;
pub const EX_DATAERR: i32 = 65;
pub const EX_UNAVAILABLE: i32 = 69;
pub const EX_SOFTWARE: i32 = 70;
pub const EX_CANTCREAT: i32 = 73;
pub const EX_TEMPFAIL: i32 = 75;
pub const EX_CONFIG: i32 = 78;

#[derive(Debug, Error)]
pub enum GovError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("temporary failure: {0}")]
    Temporary(String),
    #[error("runtime unavailable: {0}")]
    Runtime(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML decode error: {0}")]
    TomlDecode(#[from] toml::de::Error),
    #[error("TOML encode error: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl GovError {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidInput(_) => EX_USAGE,
            Self::InvalidConfig(_) | Self::TomlDecode(_) | Self::TomlEncode(_) => EX_CONFIG,
            Self::Temporary(_) => EX_TEMPFAIL,
            Self::Runtime(_) => EX_UNAVAILABLE,
            Self::Io(error) if error.kind() == io::ErrorKind::PermissionDenied => EX_CANTCREAT,
            Self::Json(_) => EX_DATAERR,
            Self::Io(_) | Self::Internal(_) => EX_SOFTWARE,
        }
    }
}

pub type Result<T> = std::result::Result<T, GovError>;
