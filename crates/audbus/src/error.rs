//! Errors of bus operations.

use std::fmt::{self, Display, Formatter};
use std::io;

use crate::name::NameError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    InvalidName(NameError),
    /// Another live transmitter owns a bus with this name.
    NameTaken,
    /// All directory slots are in use.
    DirectoryFull,
    /// No transmitter has created the bus.
    NotFound,
    /// A region was created by an incompatible protocol version.
    Incompatible,
    /// The operating system refused to create or map shared memory.
    Io(io::ErrorKind),
}

impl From<NameError> for BusError {
    fn from(error: NameError) -> Self {
        Self::InvalidName(error)
    }
}

impl From<io::Error> for BusError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::InvalidData => Self::Incompatible,
            kind => Self::Io(kind),
        }
    }
}

impl Display for BusError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(error) => write!(formatter, "{error}"),
            Self::NameTaken => formatter.write_str("Another transmitter uses this name"),
            Self::DirectoryFull => formatter.write_str("Too many buses are open"),
            Self::NotFound => formatter.write_str("No transmitter on this bus"),
            Self::Incompatible => formatter.write_str("Bus from an incompatible version"),
            Self::Io(kind) => write!(formatter, "Shared memory error: {kind}"),
        }
    }
}
