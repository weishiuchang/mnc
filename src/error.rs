use crossbeam_channel::SendError;
use thiserror::Error;

use crate::PacketBatch;

pub type Result<T> = std::result::Result<T, LibError>;

#[derive(Error, Debug)]
pub enum LibError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Nix(#[from] nix::errno::Errno),
    #[error(transparent)]
    AddrParse(#[from] std::net::AddrParseError),
    #[error(transparent)]
    SendBatch(#[from] SendError<PacketBatch>),
    #[error(transparent)]
    Any(#[from] anyhow::Error),
    #[error("{0}")]
    Critical(String),
}
