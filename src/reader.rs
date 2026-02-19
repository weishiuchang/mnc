use std::fs::File;
use std::io::{self, BufRead, BufReader, IoSliceMut};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::Sender;
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, recvmmsg};

use crate::{
    SharedState,
    error::{LibError, Result},
    multicast::{create_recv_socket, socket_to_raw_fd},
    packet::{Packet, PacketBatch, PacketType},
};

const RECVMMSG_BUFFER_COUNT: usize = 1000;
const MAX_PACKET_SIZE: usize = 65536;

/// Write batch to channel. Drop packets if channel is full.
fn write_batch_to_channel(batch: &PacketBatch, tx: &Sender<PacketBatch>) -> Result<()> {
    // This might get a bit spammy having this at warning level.
    match tx.try_send(Arc::clone(batch)) {
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

pub struct ReaderConfig {
    pub input: Option<String>,
    pub iface: Option<String>,
    pub mgroup: String,
    pub port: u16,
    pub data_tx: Sender<PacketBatch>,
    pub stats_tx: Option<Sender<PacketBatch>>,
    pub shared_state: SharedState,
    pub max_count: u64,
}

pub fn spawn(config: ReaderConfig) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        run_reader(
            config.input,
            config.iface,
            config.mgroup,
            config.port,
            config.data_tx,
            config.stats_tx,
            config.shared_state,
            config.max_count,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run_reader(
    input: Option<String>,
    iface: Option<String>,
    mgroup: String,
    port: u16,
    data_tx: Sender<PacketBatch>,
    stats_tx: Option<Sender<PacketBatch>>,
    shared_state: SharedState,
    max_count: u64,
) -> Result<()> {
    match &input {
        Some(filename) if filename == "=" => {
            log::info!("reading from stdin");
            read_from_stdin(&data_tx, &stats_tx, &shared_state, max_count)
        }
        Some(filename) => {
            log::info!("reading from {filename}");
            read_from_file(filename, &data_tx, &stats_tx, &shared_state, max_count)
        }
        None => {
            let iface_str = match &iface {
                Some(iface_str) => format!("{iface_str}:"),
                None => "".to_string(),
            };
            log::info!("reading from {iface_str}{mgroup}");
            read_from_network(
                iface,
                &mgroup,
                port,
                &data_tx,
                &stats_tx,
                &shared_state,
                max_count,
            )
        }
    }
}

pub fn read_from_network(
    iface: Option<String>,
    mgroup: &str,
    port: u16,
    data_tx: &Sender<PacketBatch>,
    stats_tx: &Option<Sender<PacketBatch>>,
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let socket = create_recv_socket(iface.as_deref(), mgroup, port)?;
    let fd = socket_to_raw_fd(&socket);

    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(RECVMMSG_BUFFER_COUNT, None);

    // Pre-allocate buffers ONCE. At high packet rates, memory allocation becomes prohibitly expensive.
    let mut packets: Vec<Packet> = (0..RECVMMSG_BUFFER_COUNT)
        .map(|_| Packet::with_capacity(MAX_PACKET_SIZE))
        .collect();

    let mut byte_counts: Vec<usize> = Vec::with_capacity(RECVMMSG_BUFFER_COUNT);

    loop {
        if shared_state.should_exit() {
            break;
        }

        if max_count > 0 && shared_state.get_count() >= max_count {
            shared_state.signal_exit();
            break;
        }

        // Reset packet buffer lengths to MAX_PACKET_SIZE for reuse
        for packet in packets.iter_mut() {
            packet.set_length(MAX_PACKET_SIZE);
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
            Err(e) => {
                if e != nix::errno::Errno::EINTR && e != nix::errno::Errno::EAGAIN {
                    log::error!("recvmmsg error: {e:?}");
                }
                continue;
            }
        }

        // Check if we have received any valid packets
        let count_received = byte_counts.iter().take_while(|&&bytes| bytes > 0).count();
        if count_received == 0 {
            continue;
        }

        // Truncate packets to actual received size in-place.
        for (idx, &bytes_received) in byte_counts.iter().enumerate().take(count_received) {
            // TODO: Figure out a performant and SAFE way to truncate
            // down to byte_received length without memory reallocation.
            unsafe {
                packets.get_unchecked_mut(idx).set_length(bytes_received);
            }
        }

        // Make sure we only send up to max packets
        let mut send_count = count_received;
        if max_count > 0 {
            let remaining = max_count - shared_state.get_count();
            if send_count as u64 > remaining {
                send_count = remaining as usize;
            }
        }

        // Clone into channel.
        let batch_packets: Vec<Packet> = packets
            .get(..send_count)
            .map(|slice| slice.iter().map(|p| Packet::new(p.to_vec())).collect())
            .unwrap_or_default();

        let batch = Arc::new(batch_packets);
        write_batch_to_channel(&batch, data_tx)?;

        if let Some(stats_tx) = stats_tx {
            write_batch_to_channel(&batch, stats_tx)?;
        }

        let already_sent = shared_state.add_count(batch.len() as u64);
        if max_count > 0 && already_sent >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}

fn read_from_file(
    filename: &str,
    data_tx: &Sender<PacketBatch>,
    stats_tx: &Option<Sender<PacketBatch>>,
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let file = File::open(filename)?;

    match shared_state.packet_type {
        PacketType::Text => read_text_mode(
            BufReader::new(file),
            data_tx,
            stats_tx,
            shared_state,
            max_count,
        ),
        _ => read_binary_mode(
            BufReader::new(file),
            data_tx,
            stats_tx,
            shared_state,
            max_count,
        ),
    }
}

fn read_from_stdin(
    data_tx: &Sender<PacketBatch>,
    stats_tx: &Option<Sender<PacketBatch>>,
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let stdin = io::stdin();

    match shared_state.packet_type {
        PacketType::Text => {
            read_text_mode(stdin.lock(), data_tx, stats_tx, shared_state, max_count)
        }
        _ => read_binary_mode(stdin.lock(), data_tx, stats_tx, shared_state, max_count),
    }
}

fn read_text_mode<R: BufRead>(
    mut reader: R,
    data_tx: &Sender<PacketBatch>,
    stats_tx: &Option<Sender<PacketBatch>>,
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let mut line = String::new();

    loop {
        if shared_state.should_exit() {
            break;
        }

        line.clear();
        let bytes_read = reader.read_line(&mut line)?;

        if bytes_read == 0 {
            break;
        }

        let packet_data = line.as_bytes().to_vec();
        let packet = Packet::new(packet_data);

        let batch = Arc::new(vec![packet]);
        write_batch_to_channel(&batch, data_tx)?;

        if let Some(stats_tx) = stats_tx {
            write_batch_to_channel(&batch, stats_tx)?;
        }

        let already_sent = shared_state.add_count(1);
        if max_count > 0 && already_sent >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}

fn read_binary_mode<R: BufRead>(
    mut reader: R,
    data_tx: &Sender<PacketBatch>,
    stats_tx: &Option<Sender<PacketBatch>>,
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        if shared_state.should_exit() {
            break;
        }

        let mut length_buf = [0u8; 4];
        match reader.read_exact(&mut length_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }

        let length = u32::from_le_bytes(length_buf) as usize;

        if length > MAX_PACKET_SIZE {
            return Err(LibError::Critical(format!(
                "Packet too large: {length} bytes"
            )));
        }

        let mut packet_data = vec![0u8; length];
        reader.read_exact(&mut packet_data)?;

        let packet = Packet::new(packet_data);
        let batch = Arc::new(vec![packet]);
        write_batch_to_channel(&batch, data_tx)?;

        if let Some(stats_tx) = stats_tx {
            write_batch_to_channel(&batch, stats_tx)?;
        }

        let already_sent = shared_state.add_count(1);
        if max_count > 0 && already_sent >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}
