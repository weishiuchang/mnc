use std::ops::Deref;
use std::sync::Arc;

// Currently we only support header parsing for these known types.
// Hopefuly we can add more in the future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PacketType {
    Text,
    Binary,
    Vita49,
    Sdds,
}

impl std::fmt::Display for PacketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PacketType::Text => write!(f, "text"),
            PacketType::Binary => write!(f, "binary"),
            PacketType::Vita49 => write!(f, "vita49"),
            PacketType::Sdds => write!(f, "sdds"),
        }
    }
}

// Generic packet type before attempting to parse as above variants
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Packet {
    data: Vec<u8>,
    length: usize,
}

impl Packet {
    pub fn new(data: Vec<u8>) -> Self {
        let length = data.len();
        Self { data, length }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity],
            length: capacity,
        }
    }

    pub fn set_length(&mut self, length: usize) {
        self.length = length
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl Deref for Packet {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // Keep our own length.
        // We have to do this to because at high packet rates,
        // memory allocation/reallocation/zero-setting drops packets.
        let len = self.length.min(self.data.len());
        match self.data.get(..len) {
            Some(slice) => slice,
            None => &[],
        }
    }
}

pub type PacketBatch = Arc<Vec<Packet>>;
