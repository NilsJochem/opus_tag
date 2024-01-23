use std::string::FromUtf8Error;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    FromUtf8(#[from] FromUtf8Error),
    #[error("only supports Version 1, but got {0}")]
    UnsupportetVersion(u8),
    #[error("{0}")]
    MalformedData(String),
    #[error("reached an EoF while expecting more data")]
    UnexpectedEoF,
    #[error("reached an EoF after a finished packet")]
    NoMoreData,
    #[error(transparent)]
    Io(std::io::Error),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::UnexpectedEof => Self::UnexpectedEoF,
            _ => Self::Io(err),
        }
    }
}
