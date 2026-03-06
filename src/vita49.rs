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

    // Read bytes 4-7 as a single big-endian u32 instead of four separate byte reads.
    // The VITA49 fields map directly onto the word:
    //   frame_sequence_number = bits 31..20  (top 12 bits)
    //   frame_size            = bits 19..0   (bottom 20 bits)
    let Some(chunk) = packet.get(4..8) else {
        return Vita49Header {
            frame_sequence_number: 0,
            frame_size: 0,
        };
    };
    let word = u32::from_be_bytes(chunk.try_into().unwrap_or([0u8; 4]));

    Vita49Header {
        frame_sequence_number: (word >> 20) as u16,
        frame_size: word & 0x000F_FFFF,
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_valid() {
        let packet = vec![b'V', b'R', b'L', b'P', 0x12, 0x34, 0x56, 0x78];

        let header = parse_header(&packet);
        assert_eq!(header.frame_sequence_number, 0x123);
        assert_eq!(header.frame_size, 0x45678);
    }
}
