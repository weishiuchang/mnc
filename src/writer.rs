use std::fs::File;
use std::io::{self, BufWriter, IoSlice, Write};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Receiver;
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, sendmmsg, sendmsg};

use crate::{
    Packet, PacketBatch, PacketType, SharedState,
    error::Result,
    multicast::{create_send_socket, socket_to_raw_fd},
};

pub struct WriterConfig {
    pub output: Option<String>,
    pub to_network: bool,
    pub iface: Option<String>,
    pub mgroup: String,
    pub port: u16,
    pub ttl: u8,
    pub data_rx: Receiver<PacketBatch>,
    pub shared_state: SharedState,
    pub rate: Option<u64>,
}

pub fn spawn(config: WriterConfig) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        run_writer(
            config.output,
            config.to_network,
            config.iface,
            config.mgroup,
            config.port,
            config.ttl,
            config.data_rx,
            config.shared_state,
            config.rate,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn run_writer(
    output: Option<String>,
    to_network: bool,
    iface: Option<String>,
    mgroup: String,
    port: u16,
    ttl: u8,
    data_rx: Receiver<PacketBatch>,
    shared_state: SharedState,
    rate: Option<u64>,
) -> Result<()> {
    match &output {
        Some(filename) if filename == "-" => {
            log::info!("writing to stdout");
            write_to_stdout(&data_rx, &shared_state)
        }
        Some(filename) => {
            log::info!("writing to {filename}");
            write_to_file(filename, &data_rx, &shared_state)
        }
        None if to_network => {
            let iface_str = match &iface {
                Some(iface_str) => format!("{iface_str}:"),
                None => "".to_string(),
            };
            log::info!("writing to {iface_str}{mgroup}");
            write_to_network(iface, &mgroup, port, ttl, &data_rx, &shared_state, rate)
        }
        None => {
            log::debug!("discarding apckets");
            write_to_devnull(&data_rx, &shared_state)
        }
    }
}

fn write_to_devnull(data_rx: &Receiver<PacketBatch>, shared_state: &SharedState) -> Result<()> {
    loop {
        match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(_) => {} // Discard packet
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if shared_state.should_exit() {
                    log::debug!("writer exiting");
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn write_to_network(
    iface: Option<String>,
    mgroup: &str,
    port: u16,
    ttl: u8,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
    rate: Option<u64>,
) -> Result<()> {
    let socket = create_send_socket(iface.as_deref(), mgroup, port, ttl)?;
    let fd = socket_to_raw_fd(&socket);

    if let Some(rate_limit) = rate {
        write_with_rate_limit(fd, data_rx, shared_state, rate_limit)
    } else {
        write_with_sendmmsg(fd, data_rx, shared_state)
    }
}

fn write_with_sendmmsg(
    fd: i32,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
) -> Result<()> {
    const BATCH_SIZE: usize = 32;
    let mut packet_batch = Vec::with_capacity(BATCH_SIZE);

    loop {
        for _ in 0..BATCH_SIZE {
            match data_rx.try_recv() {
                Ok(batch) => packet_batch.push(batch),
                Err(_) => break,
            }
        }

        if packet_batch.is_empty() {
            match data_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(batch) => packet_batch.push(batch),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if shared_state.should_exit() {
                        break;
                    }
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Flatten batches
        let packets: Vec<&Packet> = packet_batch
            .iter()
            .flat_map(|batch| batch.iter())
            .take(BATCH_SIZE)
            .collect();

        if packets.is_empty() {
            packet_batch.clear();
            continue;
        }

        let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(packets.len(), None);

        let iovecs: Vec<[IoSlice; 1]> = packets.iter().map(|pkt| [IoSlice::new(pkt)]).collect();

        match sendmmsg(fd, &mut headers, &iovecs, [], [], MsgFlags::empty()) {
            Ok(_) => {}
            Err(e) => {
                log::error!("sendmmsg error: {e:?}");
            }
        }

        packet_batch.clear();
    }

    Ok(())
}

fn write_with_rate_limit(
    fd: i32,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
    rate: u64,
) -> Result<()> {
    while let Ok(batch) = data_rx.recv() {
        for packet in batch.iter() {
            let iov = [IoSlice::new(packet)];

            match sendmsg::<()>(fd, &iov, &[], MsgFlags::empty(), None) {
                Ok(_) => {}
                Err(e) => {
                    log::error!("sendmsg error: {e:?}");
                }
            }

            for _ in 0..rate {
                std::hint::spin_loop();
            }
        }

        if shared_state.should_exit() {
            for _ in data_rx.try_iter() {}
            break;
        }
    }

    Ok(())
}

fn write_to_file(
    filename: &str,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
) -> Result<()> {
    let file = File::create(filename)?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);

    match shared_state.packet_type {
        PacketType::Text => write_text_mode(&mut writer, data_rx, shared_state),
        _ => write_binary_mode(&mut writer, data_rx, shared_state),
    }
}

fn write_to_stdout(data_rx: &Receiver<PacketBatch>, shared_state: &SharedState) -> Result<()> {
    let mut stdout = io::stdout();

    match shared_state.packet_type {
        PacketType::Text => write_text_mode(&mut stdout, data_rx, shared_state),
        _ => write_binary_mode(&mut stdout, data_rx, shared_state),
    }
}

fn write_text_mode<W: Write>(
    writer: &mut W,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
) -> Result<()> {
    while let Ok(batch) = data_rx.recv() {
        for packet in batch.iter() {
            writer.write_all(packet)?;

            if !packet.ends_with(b"\n") {
                writer.write_all(b"\n")?;
            }
        }

        if shared_state.should_exit() {
            // Drain before exiting
            for batch in data_rx.try_iter() {
                for packet in batch.iter() {
                    writer.write_all(packet)?;
                    if !packet.ends_with(b"\n") {
                        writer.write_all(b"\n")?;
                    }
                }
            }
            break;
        }
    }

    Ok(())
}

fn write_binary_mode<W: Write>(
    writer: &mut W,
    data_rx: &Receiver<PacketBatch>,
    shared_state: &SharedState,
) -> Result<()> {
    while let Ok(batch) = data_rx.recv() {
        for packet in batch.iter() {
            let length = packet.len() as u32;
            writer.write_all(&length.to_le_bytes())?;
            writer.write_all(packet)?;
        }

        if shared_state.should_exit() {
            // Drain before exiting
            for batch in data_rx.try_iter() {
                for packet in batch.iter() {
                    let length = packet.len() as u32;
                    writer.write_all(&length.to_le_bytes())?;
                    writer.write_all(packet)?;
                }
            }
            break;
        }
    }

    Ok(())
}
