use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::{
    SharedState,
    error::Result,
    packet::{PacketType, Packets},
    sdds, vita49,
};

// Print every second.
const STATISTICS_DELAY_SECS: u64 = 1;

pub struct StatisticsConfig {
    pub channels: (Receiver<Packets>, Sender<Packets>),
    pub shared_state: SharedState,
}

pub fn spawn(config: StatisticsConfig) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        run_statistics(&config)
            .inspect(|_| log::debug!("statistics exited"))
            .inspect_err(|e| {
                log::debug!("{e:?}");
                config.shared_state.signal_exit()
            })
    })
}

#[derive(Default)]
struct SddsState {
    last_seq: Option<u16>,
    skipped_in_period: u64,
    latest_timestamp: String,
}

#[derive(Default)]
struct Vita49State {
    last_seq: Option<u16>,
    skipped_in_period: u64,
}

fn run_statistics(
    StatisticsConfig {
        channels,
        shared_state,
    }: &StatisticsConfig,
) -> Result<()> {
    log::debug!("statistics for {}", &shared_state.packet_type);

    match shared_state.packet_type {
        PacketType::Text => produce_stats(
            channels,
            shared_state,
            print_hex_dump,
            |_packet, _state: &mut ()| {},
            |count, rate, _state: &()| format!("packets: {count}  rate: {rate:.2} pkt/s"),
        ),
        PacketType::Binary => produce_stats(
            channels,
            shared_state,
            print_hex_dump,
            |_packet, _state: &mut ()| {},
            |count, rate, _state: &()| format!("packets: {count}  rate: {rate:.2} pkt/s"),
        ),
        PacketType::Sdds => produce_stats(
            channels,
            shared_state,
            |packet| {
                log::info!("{}", sdds::SddsHeader::new(packet));
                print_hex_dump(packet);
            },
            |packet, state: &mut SddsState| {
                let header = sdds::parse_frame_header(packet);
                let seq = header.frame_sequence_number;
                if seq.is_multiple_of(32) {
                    state.last_seq = Some(seq);
                    return; // Every 32 packet is a parity packet
                }
                if let Some(prev_seq) = state.last_seq {
                    let expected = prev_seq.wrapping_add(1);
                    if seq != expected {
                        let skipped = if seq > expected {
                            (seq - expected) as u64
                        } else {
                            (u16::MAX - expected + seq + 1) as u64
                        };
                        state.skipped_in_period += skipped;
                    }
                }
                state.last_seq = Some(seq);
                state.latest_timestamp = sdds::format_timestamp(header.time_tag);
            },
            |count, rate, state: &SddsState| {
                let mut s = format!(
                    "packets: {count}  rate: {rate:.2} pkt/s  skipped: {}",
                    state.skipped_in_period
                );
                if !state.latest_timestamp.is_empty() {
                    s.push_str(&format!("  time: {}", state.latest_timestamp));
                }
                s
            },
        ),
        PacketType::Vita49 => produce_stats(
            channels,
            shared_state,
            |packet| {
                log::info!("{}", vita49::parse_header(packet));
                print_hex_dump(packet);
            },
            |packet, state: &mut Vita49State| {
                let header = vita49::parse_header(packet);
                let seq = header.frame_sequence_number;
                if let Some(prev_seq) = state.last_seq {
                    let expected = (prev_seq + 1) & 0xFFF;
                    if seq != expected {
                        let skipped = if seq > expected {
                            (seq - expected) as u64
                        } else {
                            0x1000 - expected as u64 + seq as u64
                        };
                        state.skipped_in_period += skipped;
                    }
                }
                state.last_seq = Some(seq);
            },
            |count, rate, state: &Vita49State| {
                format!(
                    "packets: {count}  rate: {rate:.2} pkt/s  skipped: {}",
                    state.skipped_in_period
                )
            },
        ),
    }
}

fn produce_stats<S: Default>(
    (data_rx, data_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    hex_print: impl Fn(&[u8]),
    process_packet: impl Fn(&[u8], &mut S),
    format_stats: impl Fn(u64, f64, &S) -> String,
) -> Result<()> {
    let mut last_time = Instant::now();
    let mut packet_count = 0u64;
    let mut state = S::default();

    loop {
        let packets = match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => packets,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        };

        let is_eof = packets.is_empty();

        for packet in packets.iter() {
            packet_count += 1;

            process_packet(packet, &mut state);

            if shared_state.verbose {
                hex_print(packet);
            }
        }

        // Hand off the packets to the next thread, including the eof sentinel
        data_tx.try_send(packets)?;

        let elapsed = last_time.elapsed();
        if elapsed >= Duration::from_secs(STATISTICS_DELAY_SECS) {
            let rate = packet_count as f64 / elapsed.as_secs_f64();
            log::info!("{}", format_stats(packet_count, rate, &state));

            last_time = Instant::now();
            packet_count = 0;
            state = S::default();
        }

        if is_eof || shared_state.should_exit() {
            break;
        }
    }

    Ok(())
}

// Look roughly like the output of od
fn print_hex_dump(data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let mut line = format!("{:08x}  ", i * 16);

        for (j, byte) in chunk.iter().enumerate() {
            line.push_str(&format!("{byte:02x} "));
            if j == 7 {
                line.push(' ');
            }
        }

        if chunk.len() < 16 {
            for j in chunk.len()..16 {
                line.push_str("   ");
                if j == 7 {
                    line.push(' ');
                }
            }
        }

        line.push_str(" |");
        for byte in chunk {
            let c = if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            };
            line.push(c);
        }
        line.push('|');
        log::info!("{line}");
    }
}
