use std::ops::Deref;

// Currently we only support header parsing for these types.
// Hopefuly we can add more in the future.
// We pull packet sequence number from Vita49 and Sdds and use
// it in the statistics thread to count drops.
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

/// Generic packet type before attempting to parse as above variants
/// The underlying buffer is pre-allocated and reused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Packet {
    data: Vec<u8>,
    length: usize,
}

impl Packet {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity],
            length: capacity,
        }
    }

    pub fn set_length(&mut self, length: usize) {
        self.length = length
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl Deref for Packet {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // Keep our own length.
        // We have to do this to because at high packet rates (> 100k pps),
        // memory allocation/reallocation/zero-setting drops packets.
        let len = self.length.min(self.data.len());
        match self.data.get(..len) {
            Some(slice) => slice,
            None => &[],
        }
    }
}

/// Collection of packets that will be recycled through the channels pre-allocated with a fix
/// number of packets, each with max udp length.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Packets {
    packets: Vec<Packet>,
    length: usize,
}

impl Packets {
    /// Allocate a new batch of packets
    pub fn new(length: usize, per_packet_length: usize) -> Self {
        let packets = (0..length)
            .map(|_| Packet::with_capacity(per_packet_length))
            .collect();
        Self { packets, length }
    }

    // sentinel value. indicating EOF
    pub fn empty() -> Self {
        Self {
            packets: Vec::new(),
            length: 0,
        }
    }

    #[allow(clippy::indexing_slicing)]
    pub fn packets_mut(&mut self) -> &mut [Packet] {
        &mut self.packets[..self.length]
    }

    /// Set the number of valid packets in in this batch
    pub fn set_length(&mut self, length: usize) {
        self.length = length.min(self.packets.len());
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    #[allow(clippy::indexing_slicing)]
    pub fn iter(&self) -> impl Iterator<Item = &Packet> {
        self.packets[..self.length].iter()
    }

    #[allow(clippy::indexing_slicing)]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Packet> {
        self.packets[..self.length].iter_mut()
    }
}
