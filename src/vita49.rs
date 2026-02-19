pub const HEADER_SIZE: usize = 8;

pub struct Vita49Header {
    pub frame_sequence_number: u16,
    pub frame_size: u32,
}

impl std::fmt::Display for Vita49Header {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "VITA49 Header:")?;
        writeln!(f, "  {:<24}:  VRLP", "Identifier")?;
        writeln!(
            f,
            "  {:<24}: {:<25} {:012b}",
            "Frame Sequence (12)", self.frame_sequence_number, self.frame_sequence_number
        )?;
        writeln!(
            f,
            "  {:<24}: {:<25} {:020b}",
            "Frame Size (20)", self.frame_size, self.frame_size
        )
    }
}

pub fn parse_header(packet: &[u8]) -> Vita49Header {
    if packet.len() < HEADER_SIZE {
        return Vita49Header {
            frame_sequence_number: 0,
            frame_size: 0,
        };
    }

    if packet.get(0..4) != Some(b"VRLP") {
        return Vita49Header {
            frame_sequence_number: 0,
            frame_size: 0,
        };
    }

    let byte4 = packet.get(4).map(|&b| u16::from(b)).unwrap_or(0);
    let byte5 = packet.get(5).map(|&b| u16::from(b)).unwrap_or(0);
    let byte6 = packet.get(6).map(|&b| u32::from(b)).unwrap_or(0);
    let byte7 = packet.get(7).map(|&b| u32::from(b)).unwrap_or(0);

    let frame_sequence_number = (byte4 << 4) | (byte5 >> 4);

    let frame_size = (u32::from(byte5 & 0x0F) << 16) | (byte6 << 8) | byte7;

    Vita49Header {
        frame_sequence_number,
        frame_size,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_valid() {
        let packet = vec![b'V', b'R', b'L', b'P', 0x12, 0x34, 0x56, 0x78];

        let header = parse_header(&packet);
        assert_eq!(header.frame_sequence_number, 0x123);
        assert_eq!(header.frame_size, 0x4567);
    }
}
