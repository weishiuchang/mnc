use thiserror::Error;

use crate::Packets;

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
    SendBatch(#[from] crossbeam_channel::SendError<Packets>),
    #[error(transparent)]
    TrySendBatch(#[from] crossbeam_channel::TrySendError<Packets>),
    #[error(transparent)]
    RecvBatch(#[from] crossbeam_channel::RecvError),
    #[error(transparent)]
    TryRecvBatch(#[from] crossbeam_channel::TryRecvError),
    #[error(transparent)]
    RecvTimeoutBatch(#[from] crossbeam_channel::RecvTimeoutError),
    #[error("{0}")]
    Critical(String),
}
