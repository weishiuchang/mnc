/// Important: Ensure we don't drop the Packets, it must recycle
/// through the memory channel back to the reader thread.
use std::fs::File;
use std::io::{self, BufWriter, IoSlice, Write};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, sendmmsg, sendmsg};

use crate::{
    SharedState,
    error::Result,
    multicast::{create_send_socket, socket_to_raw_fd},
    packet::{PacketType, Packets},
};

pub struct WriterConfig {
    pub output: Option<String>,
    pub to_network: bool,
    pub iface: Option<String>,
    pub mgroup: String,
    pub port: u16,
    pub ttl: u8,
    pub channels: (Receiver<Packets>, Sender<Packets>),
    pub shared_state: SharedState,
    pub rate: Option<u64>,
    pub max_count: u64,
}

pub fn spawn(config: WriterConfig) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        run_writer(&config)
            .inspect(|_| log::debug!("writer exited"))
            .inspect_err(|e| {
                log::error!("{e:?}");
                config.shared_state.signal_exit()
            })
    })
}

fn run_writer(
    WriterConfig {
        output,
        to_network,
        iface,
        mgroup,
        port,
        ttl,
        channels,
        shared_state,
        rate,
        max_count,
    }: &WriterConfig,
) -> Result<()> {
    match &output {
        Some(filename) if filename == "-" => {
            log::info!("writing to stdout");
            write_to_stdout(channels, shared_state, *max_count)
        }
        Some(filename) => {
            log::info!("writing to {filename}");
            write_to_file(filename, channels, shared_state, *max_count)
        }
        None if *to_network => {
            let iface_str = match iface {
                Some(iface_str) => format!("{iface_str}:"),
                None => "".to_string(),
            };
            log::info!("writing to {iface_str}{mgroup}");
            write_to_network(
                iface.as_deref(),
                mgroup,
                *port,
                *ttl,
                channels,
                shared_state,
                *rate,
                *max_count,
            )
        }
        None => {
            log::debug!("discarding packets");
            write_to_devnull(channels, shared_state, *max_count)
        }
    }
}

fn write_to_devnull(
    (data_rx, memory_return_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        if shared_state.should_exit() {
            break;
        }

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }

        match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => {
                if packets.is_empty() {
                    // EOF signal
                    break;
                }

                // Calculate how many packets to process
                let mut process_count = packets.len();
                if max_count > 0 {
                    let remaining = max_count - shared_state.get_write_count();
                    if process_count as u64 > remaining {
                        process_count = remaining as usize;
                    }
                }

                shared_state.add_write_count(process_count as u64);

                // Return packets back to memory pool
                memory_return_tx.try_send(packets)?;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_to_network(
    iface: Option<&str>,
    mgroup: &str,
    port: u16,
    ttl: u8,
    channels: &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    rate: Option<u64>,
    max_count: u64,
) -> Result<()> {
    let socket = create_send_socket(iface, mgroup, port, ttl)?;
    let fd = socket_to_raw_fd(&socket);

    if let Some(rate_limit) = rate {
        write_with_rate_limit(fd, channels, shared_state, rate_limit, max_count)
    } else {
        write_with_sendmmsg(fd, channels, shared_state, max_count)
    }
}

fn write_with_sendmmsg(
    fd: i32,
    (data_rx, memory_return_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        let packets = match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => packets,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        };

        if shared_state.should_exit() {
            break;
        }

        if packets.is_empty() {
            // EOF signal
            break;
        }

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }

        // Calculate how many packets to send
        let mut send_count = packets.len();
        if max_count > 0 {
            let remaining = max_count - shared_state.get_write_count();
            if send_count as u64 > remaining {
                send_count = remaining as usize;
            }
        }

        let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(send_count, None);

        let iovecs: Vec<[IoSlice; 1]> = packets
            .iter()
            .take(send_count)
            .map(|pkt| [IoSlice::new(pkt)])
            .collect();

        sendmmsg(fd, &mut headers, &iovecs, [], [], MsgFlags::empty())?;

        shared_state.add_write_count(send_count as u64);

        // Return packets to memory pool
        memory_return_tx.try_send(packets)?;

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }
    }

    Ok(())
}

fn write_with_rate_limit(
    fd: i32,
    (data_rx, memory_return_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    rate: u64,
    max_count: u64,
) -> Result<()> {
    loop {
        let packets = match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => packets,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        };

        if shared_state.should_exit() {
            break;
        }

        // Check for EOF
        if packets.is_empty() {
            break;
        }

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }

        // Calculate how many packets to send
        let mut send_limit = packets.len();
        if max_count > 0 {
            let remaining = max_count - shared_state.get_write_count();
            if send_limit as u64 > remaining {
                send_limit = remaining as usize;
            }
        }

        let mut sent_count = 0u64;
        for (idx, packet) in packets.iter().enumerate() {
            if idx >= send_limit {
                break;
            }

            let iov = [IoSlice::new(packet)];

            sendmsg::<()>(fd, &iov, &[], MsgFlags::empty(), None).inspect(|_| sent_count += 1)?;

            for _ in 0..rate {
                std::hint::spin_loop();
            }
        }

        shared_state.add_write_count(sent_count);

        // Return batch to memory pool
        memory_return_tx.try_send(packets)?;

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}

fn write_to_file(
    filename: &str,
    channels: &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let file = File::create(filename)?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);

    match shared_state.packet_type {
        PacketType::Text => write_text_mode(&mut writer, channels, shared_state, max_count),
        _ => write_binary_mode(&mut writer, channels, shared_state, max_count),
    }
}

fn write_to_stdout(
    channels: &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    let mut stdout = io::stdout();

    match shared_state.packet_type {
        PacketType::Text => write_text_mode(&mut stdout, channels, shared_state, max_count),
        _ => write_binary_mode(&mut stdout, channels, shared_state, max_count),
    }
}

fn write_text_mode<W: Write>(
    writer: &mut W,
    (data_rx, memory_return_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        let packets = match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => packets,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        };

        if shared_state.should_exit() {
            break;
        }

        // Check for EOF
        if packets.is_empty() {
            break;
        }

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }

        // Calculate how many packets to write
        let mut write_limit = packets.len();
        if max_count > 0 {
            let remaining = max_count - shared_state.get_write_count();
            if write_limit as u64 > remaining {
                write_limit = remaining as usize;
            }
        }

        for (idx, packet) in packets.iter().enumerate() {
            if idx >= write_limit {
                break;
            }

            writer.write_all(packet)?;

            if !packet.ends_with(b"\n") {
                writer.write_all(b"\n")?;
            }
        }

        shared_state.add_write_count(write_limit as u64);

        // Return batch to memory pool
        memory_return_tx.try_send(packets)?;

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}

fn write_binary_mode<W: Write>(
    writer: &mut W,
    (data_rx, memory_return_tx): &(Receiver<Packets>, Sender<Packets>),
    shared_state: &SharedState,
    max_count: u64,
) -> Result<()> {
    loop {
        let packets = match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(packets) => packets,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        };

        if shared_state.should_exit() {
            break;
        }

        // Check for EOF
        if packets.is_empty() {
            break;
        }

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            break;
        }

        // Calculate how many packets to write
        let mut write_limit = packets.len();
        if max_count > 0 {
            let remaining = max_count - shared_state.get_write_count();
            if write_limit as u64 > remaining {
                write_limit = remaining as usize;
            }
        }

        for (idx, packet) in packets.iter().enumerate() {
            if idx >= write_limit {
                break;
            }

            let length = packet.len() as u32;
            writer.write_all(&length.to_le_bytes())?;
            writer.write_all(packet)?;
        }

        shared_state.add_write_count(write_limit as u64);

        // Return packets to memory pool
        memory_return_tx.try_send(packets)?;

        if max_count > 0 && shared_state.get_write_count() >= max_count {
            shared_state.signal_exit();
            break;
        }
    }

    Ok(())
}
