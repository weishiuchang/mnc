// Eth Frame //   SDDS data object: 1138 bytes total
// Eth frame // bytes
// Eth Frame //   8            Preamble
// Eth Frame //   18           Ethernet header
// Eth Frame //  *1108         [IP Packet]
// Eth Frame //   4            Ethernet FCS
// Eth Frame //
// Eth Frame //   IP Packet: 1108 bytes
// Eth Frame // bytes
// Eth Frame //   20           IP Header
// Eth Frame //   8            UDP Header
// Eth Frame //  *1080         [Payload]
//
//   Payload: 1080 bytes
// bytes
//  *2           Format Identifier
//   2           Frame sequence #
//  *16          Time Tag
//  *12
//   4
//   20          reserved
//   1024        [Data]

pub fn frame_sequence_number(packet: &[u8]) -> u16 {
    packet
        .get(2..4)
        .and_then(|b| b.try_into().ok())
        .map(u16::from_be_bytes)
        .unwrap_or(0)
}

pub fn time_tag(packet: &[u8]) -> u64 {
    packet
        .get(8..16)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_be_bytes)
        .unwrap_or(0)
}

pub fn time_tag_ext(packet: &[u8]) -> u32 {
    packet
        .get(16..20)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_be_bytes)
        .unwrap_or(0)
}

pub fn sddstime(timetag: u64) -> (u32, u32, u32, u32, u64) {
    let mut tt = timetag;

    let nsecs = (tt % 4_000_000_000) / 4;
    tt /= 4_000_000_000;

    let secs = (tt % 60) as u32;
    tt /= 60;

    let mins = (tt % 60) as u32;
    tt /= 60;

    let hours = (tt % 24) as u32;
    tt /= 24;

    let days = 1 + tt as u32;

    (days, hours, mins, secs, nsecs)
}

pub fn format_timestamp(timetag: u64) -> String {
    let (days, hours, mins, secs, nsecs) = sddstime(timetag);
    format!("{days:03}:{hours:02}:{mins:02}:{secs:02}:{nsecs:09}")
}

pub fn sf(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| (b & 0x80) != 0)
}

pub fn sos(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| (b & 0x40) != 0)
}

pub fn pp(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| (b & 0x20) != 0)
}

pub fn of(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| (b & 0x10) != 0)
}

pub fn ss(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| (b & 0x08) != 0)
}

pub fn data_mode(packet: &[u8]) -> u8 {
    packet.first().map_or(0, |b| b & 0x07)
}

pub fn cx(packet: &[u8]) -> bool {
    packet.get(1).is_some_and(|b| (b & 0x80) != 0)
}

pub fn snp(packet: &[u8]) -> bool {
    packet.get(1).is_some_and(|b| (b & 0x40) != 0)
}

pub fn vw(packet: &[u8]) -> bool {
    packet.get(1).is_some_and(|b| (b & 0x20) != 0)
}

pub fn bits_per_sample(packet: &[u8]) -> u8 {
    packet.get(1).map_or(0, |b| b & 0x1F)
}

pub struct SddsHeader<'a> {
    packet: &'a [u8],
}

impl<'a> SddsHeader<'a> {
    pub fn new(packet: &'a [u8]) -> Self {
        Self { packet }
    }
}

impl std::fmt::Display for SddsHeader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let seq = frame_sequence_number(self.packet);
        let timetag = time_tag(self.packet);
        let tt_ext = time_tag_ext(self.packet);

        writeln!(f, "SDDS Header:")?;
        writeln!(f, "  {:24}: {seq:<25} {seq:016b}", "Frame Sequence (16)")?;
        writeln!(
            f,
            "  {:24}: {:<25} {timetag:064b}",
            "Time Tag (64)",
            format_timestamp(timetag),
        )?;
        writeln!(f, "  {:24}: {:<25} {tt_ext:032b}", "Time Tag Ext (32)", " ")?;
        writeln!(
            f,
            "    {:22}: {:<25} {}",
            "SF (1)",
            sf(self.packet) as u8,
            sf(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<26} {}",
            "SoS(1)",
            sos(self.packet) as u8,
            sos(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<27} {}",
            "PP (1)",
            pp(self.packet) as u8,
            pp(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<28} {}",
            "OF (1)",
            of(self.packet) as u8,
            of(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<29} {}",
            "SS (1)",
            ss(self.packet) as u8,
            ss(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<30} {:03b}",
            "Data Mode (3)",
            { data_mode(self.packet) },
            { data_mode(self.packet) }
        )?;
        writeln!(
            f,
            "    {:22}: {:<33} {}",
            "CX (1)",
            cx(self.packet) as u8,
            cx(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<34} {}",
            "SNP (1)",
            snp(self.packet) as u8,
            snp(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<35} {}",
            "VW (1)",
            vw(self.packet) as u8,
            vw(self.packet) as u8
        )?;
        writeln!(
            f,
            "    {:22}: {:<36} {:05b}",
            "Bits per Sample (5)",
            { bits_per_sample(self.packet) },
            { bits_per_sample(self.packet) }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_sequence_number() {
        let packet = vec![0, 0, 0x12, 0x34];
        assert_eq!(frame_sequence_number(&packet), 0x1234);
    }

    #[test]
    fn test_time_tag() {
        let packet = vec![0; 16];
        if let Some(slice) = packet.get_mut(8..16) {
            slice.copy_from_slice(&0x0123456789ABCDEFu64.to_be_bytes());
        }
        assert_eq!(time_tag(&packet), 0x0123456789ABCDEF);
    }

    #[test]
    fn test_sddstime() {
        let (days, hours, mins, secs, nsecs) = sddstime(0);
        assert_eq!((days, hours, mins, secs, nsecs), (1, 0, 0, 0, 0));

        let one_day = 4_000_000_000u64 * 60 * 60 * 24;
        let (days, hours, mins, secs, nsecs) = sddstime(one_day);
        assert_eq!((days, hours, mins, secs, nsecs), (2, 0, 0, 0, 0));
    }

    #[test]
    fn test_format_identifier() {
        let packet = vec![0b10110101, 0b11010111];
        assert_eq!(sf(&packet), true);
        assert_eq!(sos(&packet), false);
        assert_eq!(pp(&packet), true);
        assert_eq!(of(&packet), true);
        assert_eq!(ss(&packet), false);
        assert_eq!(data_mode(&packet), 0b101);
        assert_eq!(cx(&packet), true);
        assert_eq!(snp(&packet), true);
        assert_eq!(vw(&packet), false);
        assert_eq!(bits_per_sample(&packet), 0b10111);
    }
}
