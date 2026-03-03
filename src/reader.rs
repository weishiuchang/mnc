/// The reader thread pulls Packets from a memory pool initially.
/// The Packets are the recycled through the writer thread to
/// sidestep memory allocation as it is a large performance hit.
use std::fs::File;
use std::io::{self, BufRead, BufReader, IoSliceMut};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, recvmmsg};

use crate::{
    MAX_PACKET_BYTES, PACKETS_BATCH_SIZE, SharedState,
    error::{LibError, Result},
    multicast::{create_recv_socket, socket_to_raw_fd},
    packet::{PacketType, Packets},
};

pub struct ReaderConfig {
    pub input: Option<String>,
    pub iface: Option<String>,
    pub mgroup: String,
    pub port: u16,
    pub channels: (Sender<Packets>, Receiver<Packets>),
    pub shared_state: SharedState,
    pub max_count: u64,
}

pub fn spawn(config: ReaderConfig) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        run_reader(&config)
            .inspect(|_| log::debug!("reader exited"))
            .inspect_err(|e| {
                log::debug!("{e:?}");
                config.shared_state.signal_exit()
            })
    })
}

pub fn run_reader(
    ReaderConfig {
        input,
        iface,
        mgroup,
        port,
        channels,
        shared_state,
        max_count,
    }: &ReaderConfig,
) -> Result<()> {
    match &input {
        Some(filename) if filename == "=" => {
            log::info!("reading from stdin");
            read_from_stdin(channels, shared_state, *max_count)
        }
        Some(filename) => {
            log::info!("reading from {filename}");
            read_from_file(filename, channels, shared_state, *max_count)
        }
        None => {
            let iface_str = match iface {
                Some(iface_str) => format!("{iface_str}:"),
                None => "".to_string(),
            };
            log::info!("reading from {iface_str}{mgroup}");
            read_from_network(
                iface.as_deref(),
                mgroup,
                *port,
                channels,
                shared_state,
                *max_count,
            )
        }
    }
}

pub fn read_from_network(
    iface: Option<&str>,
    mgroup: &str,
    port: u16,
    (data_tx, memory_return_rx): &(Sender<Packets>, Receiver<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let socket = create_recv_socket(iface, mgroup, port)?;
    let fd = socket_to_raw_fd(&socket);

    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(PACKETS_BATCH_SIZE, None);
    let mut byte_counts: Vec<usize> = Vec::with_capacity(PACKETS_BATCH_SIZE);

    loop {
        // Pull a recycled Packets from the memory pool (blocking)
        let mut packets = memory_return_rx.recv()?;

        if max_count > 0 && shared_state.get_read_count() >= max_count {
            // Send empty packets to signal EOF
            // Do not signal_exit() to give the other threads a chance
            // to finish processing what's left in the channels.
            packets.set_length(0);
            write_packets_to_channel(packets, data_tx)?;
            break;
        }

        // Reset packet buffer lengths to MAX_PACKET_BYTES for reuse
        for packet in packets.iter_mut() {
            packet.set_length(MAX_PACKET_BYTES);
        }

        // Create iovecs pointing to our persisten buffers.
        let mut iovecs: Vec<[IoSliceMut; 1]> = packets
            .iter_mut()
            .map(|packet| [IoSliceMut::new(packet.data_mut())])
            .collect();

        byte_counts.clear();
        match recvmmsg(
            fd,
            &mut headers,
            &mut iovecs,
            MsgFlags::MSG_WAITFORONE,
            None,
        ) {
            Ok(msgs) => {
                byte_counts.extend(msgs.into_iter().map(|msg| msg.bytes));
            }
            Err(nix::errno::Errno::EAGAIN) => {
                // Retry on EAGAIN
            }
            Err(e) => return Err(e.into()),
        }

        if shared_state.should_exit() {
            break;
        }

        let count_received = byte_counts.iter().take_while(|&&bytes| bytes > 0).count();

        // Make sure we only send up to user specified max packets
        let mut send_count = count_received;
        if max_count > 0 {
            let remaining = max_count - shared_state.get_read_count();
            if send_count as u64 > remaining {
                send_count = remaining as usize;
            }
        }

        packets.set_length(send_count);

        // Set each packet length to what recvmmsg tells us
        #[allow(clippy::indexing_slicing)]
        for (idx, &bytes_received) in byte_counts.iter().enumerate().take(send_count) {
            packets.packets_mut()[idx].set_length(bytes_received);
        }

        if !packets.is_empty() {
            // Send to next thread
            write_packets_to_channel(packets, data_tx)?;
        }

        let already_sent = shared_state.add_read_count(send_count as u64);
        if max_count > 0 && already_sent >= max_count {
            // Send empty packets to signal EOF
            write_packets_to_channel(Packets::empty(), data_tx)?;
            break;
        }
    }

    Ok(())
}

fn read_from_file(
    filename: &str,
    channels: &(Sender<Packets>, Receiver<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let file = File::open(filename)?;

    match shared_state.packet_type {
        PacketType::Text => read_text_mode(BufReader::new(file), channels, shared_state, max_count),
        _ => read_binary_mode(BufReader::new(file), channels, shared_state, max_count),
    }
}

fn read_from_stdin(
    channels: &(Sender<Packets>, Receiver<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let stdin = io::stdin();

    match shared_state.packet_type {
        PacketType::Text => read_text_mode(stdin.lock(), channels, shared_state, max_count),
        _ => read_binary_mode(stdin.lock(), channels, shared_state, max_count),
    }
}

fn read_text_mode<R: BufRead>(
    mut reader: R,
    (data_tx, memory_return_rx): &(Sender<Packets>, Receiver<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let mut line = String::new();

    loop {
        // Pull a recycled Packets from the memory pool (blocking)
        let mut packets = memory_return_rx.recv()?;

        line.clear();
        let bytes_read = reader.read_line(&mut line)?;

        if shared_state.should_exit() {
            break;
        }

        if bytes_read == 0 {
            // EOF - send empty packets sentinel
            packets.set_length(0);
            write_packets_to_channel(packets, data_tx)?;
            break;
        }

        let packet_data = line.as_bytes();
        #[allow(clippy::indexing_slicing)]
        {
            packets.packets_mut()[0].data_mut()[..packet_data.len()].copy_from_slice(packet_data);
            packets.packets_mut()[0].set_length(packet_data.len());
        }
        packets.set_length(1);

        write_packets_to_channel(packets, data_tx)?;

        let already_sent = shared_state.add_read_count(1);
        if max_count > 0 && already_sent >= max_count {
            // Send empty packets to signal EOF
            write_packets_to_channel(Packets::empty(), data_tx)?;
            break;
        }
    }

    Ok(())
}

fn read_binary_mode<R: BufRead>(
    mut reader: R,
    (data_tx, memory_return_rx): &(Sender<Packets>, Receiver<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        // Pull a recycled Packets from the memory pool (blocking)
        let mut packets = memory_return_rx.recv()?;

        // u64 for packet length is overkill, but I've learned the value of giving
        // myself some room for future things.
        let mut length_buf = [0u8; 4];
        reader.read_exact(&mut length_buf)?;

        if shared_state.should_exit() {
            break;
        }

        let length = u32::from_le_bytes(length_buf) as usize;

        if length > MAX_PACKET_BYTES {
            return Err(LibError::Critical(format!(
                "Packet too large: {length} bytes"
            )));
        }

        // Read into the first packet
        #[allow(clippy::indexing_slicing)]
        {
            reader.read_exact(&mut packets.packets_mut()[0].data_mut()[..length])?;
            packets.packets_mut()[0].set_length(length);
        }
        packets.set_length(1);

        if shared_state.should_exit() {
            break;
        }

        write_packets_to_channel(packets, data_tx)?;

        let already_sent = shared_state.add_read_count(1);
        if max_count > 0 && already_sent >= max_count {
            // Send empty packets to signal EOF
            write_packets_to_channel(Packets::empty(), data_tx)?;
            break;
        }
    }

    Ok(())
}

/// Write packets to channel. Drop packets if channel is full.
fn write_packets_to_channel(packets: Packets, tx: &Sender<Packets>) -> Result<()> {
    // This might get a bit spammy having this at warning level.
    match tx.try_send(packets) {
        Ok(()) => {}
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            log::warn!("dropping packets");
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            return Err(LibError::Critical("channel disconnected".to_string()));
        }
    }

    Ok(())
}
