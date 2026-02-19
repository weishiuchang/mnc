// Single concern main.
// Make sure we manage the startup and shutdown of subordinate threads.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use clap::Parser;
use crossbeam_channel::{Receiver, Sender, bounded};
use regex::Regex;

use packet::{PacketType, Packets};

/// Max UDP Packet size in bytes
const MAX_PACKET_BYTES: usize = 65536;
/// How many packets in each batch
const PACKETS_BATCH_SIZE: usize = 1000;

mod error;
mod multicast;
mod packet;
mod reader;
mod sdds;
mod statistics;
mod vita49;
mod writer;

#[derive(Parser)]
#[command(name = "mnc")]
#[command(about = "Multicast netcat - CLI utility for sending and receiving multicast packets")]
#[command(after_help = "EXAMPLES:
  # Receive from multicast group and display text payload
  mnc 239.1.1.1

  # Send string to multcast group as a single packet
  echo \"Hello World\" | mnc 239.1.1.1.1 -i -

  # Receive \"Hello World\" on eth1
  mnc eth1:239.1.1.1

  # Receive and display statistics every 2 seconds
  mnc 239.1.1.1 -s

  # Display a hex dump of the first packet received
  mnc 239.1.1.1 -v

  # Receive exactly 10 packets then exit
  mnc 239.1.1.1 -c 10

  # Send file contents to multicast group, each line a packet
  mnc 239.1.1.1 -i ./file.txt

  # Save multicast to file
  mnc 239.1.1.1 -o ./output.txt

  # Show periodic SDDS statistics
  mnc 239.1.1.1 -t sdds -s

  # Show periodic VITA49 statistics with given port
  mnc 239.1.1.1 -p 12345 -t vita49 -s")]
struct Args {
    #[arg(value_parser = parse_mgroup, help = "[eth:]mgroup")]
    mgroup: (Option<String>, String),

    #[arg(
        short = 't',
        long = "type",
        default_value = "text",
        help = "Multicast packet type"
    )]
    packet_type: PacketType,

    #[arg(
        short = 'i',
        long = "input",
        help = "Read packets from filename, or - for stdin"
    )]
    input: Option<String>,

    #[arg(
        short = 'o',
        long = "output",
        help = "Write packets to filename, or - for stdout"
    )]
    output: Option<String>,

    #[arg(
        short = 's',
        long = "statistics",
        help = "Display statistics every 2 seconds"
    )]
    stats: bool,

    #[arg(
        short = 'p',
        long = "port",
        default_value = "29495",
        help = "Multicast port"
    )]
    port: u16,

    #[arg(
        short = 'b',
        long = "buffer-size",
        default_value = "10000",
        help = "In-memory buffer capacity"
    )]
    buffer_size: usize,

    #[arg(
        short = 'L',
        long = "ttl",
        default_value = "255",
        help = "Multicast hop limit"
    )]
    ttl: u8,

    #[arg(short = 'q', long = "quiet", help = "Quiet mode: suppress all output")]
    quiet: bool,

    #[arg(
        short = 'c',
        long = "count",
        help = "Exit after receiving/sending count packets (0 = no limit)"
    )]
    count: Option<u64>,

    #[arg(
        short = 'r',
        long = "rate",
        help = "Rate limit by adding rate noop instructions between sendmsg calls"
    )]
    rate: Option<u64>,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Hex dump UDP payloads (assumes -c1 if not specified)"
    )]
    verbose: bool,

    #[arg(short = 'd', long = "debug", help = "Enable debug logging")]
    debug: bool,
}

// Some global variables to help control thread shutdown.
#[derive(Clone)]
pub struct SharedState {
    pub read_count: Arc<AtomicU64>,
    pub write_count: Arc<AtomicU64>,
    /// Exit conditions:
    /// - should_exit is immediate: ctrl-c and errors.
    /// - any other normal exit is indicated by an empty packet batch (sentinel value)
    pub should_exit: Arc<AtomicBool>,
    pub packet_type: PacketType,
    pub verbose: bool,
}

impl SharedState {
    fn new(packet_type: PacketType, verbose: bool) -> Self {
        Self {
            read_count: Arc::new(AtomicU64::new(0)),
            write_count: Arc::new(AtomicU64::new(0)),
            should_exit: Arc::new(AtomicBool::new(false)),
            packet_type,
            verbose,
        }
    }

    pub fn add_read_count(&self, delta: u64) -> u64 {
        self.read_count.fetch_add(delta, Ordering::Relaxed) + delta
    }
    pub fn get_read_count(&self) -> u64 {
        self.read_count.load(Ordering::Relaxed)
    }
    pub fn add_write_count(&self, delta: u64) -> u64 {
        self.write_count.fetch_add(delta, Ordering::Relaxed) + delta
    }
    pub fn get_write_count(&self) -> u64 {
        self.write_count.load(Ordering::Relaxed)
    }
    pub fn signal_exit(&self) {
        self.should_exit.store(true, Ordering::Relaxed);
    }
    pub fn should_exit(&self) -> bool {
        self.should_exit.load(Ordering::Relaxed)
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // If verbose is set and user didn't specify count, default to 1
    let max_count = match (args.verbose, args.count) {
        (true, None) => 1,  // verbose without explicit count
        (_, Some(c)) => c,  // user specified count takes precedence
        (false, None) => 0, // neither verbose nor count specified
    };

    let log_level = if args.debug {
        "debug"
    } else if args.quiet {
        "warn"
    } else {
        "info"
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .target(env_logger::Target::Stdout)
        .init();

    // Exit toggles for threads
    let shared_state = SharedState::new(args.packet_type, args.verbose);
    let mut all_threads: Vec<_> = Vec::new();

    // Memory return channel: Writer -> Reader for packet recycling
    const PACKET_BATCH_SIZE: usize = 1000;
    let (memory_return_tx, memory_return_rx): (Sender<Packets>, Receiver<Packets>) =
        bounded(PACKET_BATCH_SIZE);

    // Memory allocation is expensive at high pps, so do this in a background thread.
    initialize_memory_pool(
        memory_return_tx.clone(),
        PACKET_BATCH_SIZE,
        shared_state.clone(),
    );

    // Reader -> [Statistics] -> Writer -> Reader (memory return)
    let (reader_tx, reader_rx) = bounded(args.buffer_size);
    let writer_rx = if !args.quiet && (args.stats || args.verbose) {
        let (stats_tx, stats_rx) = bounded(args.buffer_size);

        // Statistics gives us some useful information about the packets
        log::debug!("spawning statistics thread");
        let handle = statistics::spawn(statistics::StatisticsConfig {
            channels: (reader_rx, stats_tx),
            shared_state: shared_state.clone(),
        });

        all_threads.push(handle);

        stats_rx
    } else {
        reader_rx
    };

    // Writer sends packets to network/file/stdout. Discards all packets by default.
    log::debug!("spawning writer thread");
    let writer_handle = writer::spawn(writer::WriterConfig {
        output: args.output.clone(),
        to_network: args.input.is_some(),
        iface: args.mgroup.0.clone(),
        mgroup: args.mgroup.1.clone(),
        port: args.port,
        ttl: args.ttl,
        channels: (writer_rx, memory_return_tx),
        shared_state: shared_state.clone(),
        rate: args.rate,
        max_count,
    });
    all_threads.push(writer_handle);

    // Reader pulls packets from network/file/stdin
    log::debug!("spawning reader thread");
    let reader_handle = reader::spawn(reader::ReaderConfig {
        input: args.input.clone(),
        iface: args.mgroup.0.clone(),
        mgroup: args.mgroup.1.clone(),
        port: args.port,
        channels: (reader_tx, memory_return_rx),
        shared_state: shared_state.clone(),
        max_count,
    });
    all_threads.push(reader_handle);

    let ctrl_c = shared_state.clone();
    ctrlc::set_handler(move || {
        log::debug!("Exiting...");
        ctrl_c.signal_exit();
    })?;

    let mut exiting_timeout: Option<std::time::Instant> = None;
    loop {
        // Wait at most 1s if exit has been signaled.
        if shared_state.should_exit() {
            if exiting_timeout
                .is_some_and(|timeout| timeout.elapsed() > std::time::Duration::from_secs(1))
            {
                log::debug!("Timed out waiting 1s for threads");
                return Err(anyhow::anyhow!("Exiting"));
            } else {
                exiting_timeout = Some(std::time::Instant::now());
            }
        }

        let mut still_running = Vec::new();
        for handle in all_threads.into_iter() {
            // Non-blocking check if thread has finished
            if handle.is_finished() {
                handle
                    .join()
                    .map_err(|e| error::LibError::Critical(format!("join error: {e:?}")))??;
                shared_state.signal_exit();
            } else {
                still_running.push(handle);
            }
        }
        all_threads = still_running;

        if all_threads.is_empty() {
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Ok(())
}

// Parse [eth:]mgroup into (eth, mgroup)
fn parse_mgroup(s: &str) -> std::result::Result<(Option<String>, String), String> {
    let mgroup_regex =
        Regex::new(r"^(?:(?P<iface>[^:]+):)?(?P<mgroup>\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})$")
            .map_err(|e| format!("Regex compilation error: {e:?}"))?;

    let caps = mgroup_regex
        .captures(s)
        .ok_or_else(|| format!("Expected [eth:]mgroup, got: {s}"))?;

    let iface = caps.name("iface").map(|m| m.as_str().to_string());

    let mgroup = caps
        .name("mgroup")
        .ok_or_else(|| format!("Not a multicast address: {s}"))?
        .as_str();

    Ok((iface, mgroup.to_string()))
}

// Push some packets into the memory return channel.
// This will get read by the reader and data populated.
// Note: We are minimizing memory (re)allocations, so
// make sure the packets that are injected into are not dropped
// except on exit.
fn initialize_memory_pool(
    memory_return_tx: Sender<Packets>,
    pool_size: usize,
    shared_state: SharedState,
) {
    std::thread::spawn(move || {
        log::debug!("allocating memory pool with {pool_size} buffers");

        for i in 0..pool_size {
            if shared_state.should_exit() {
                log::debug!("memory pool thread exiting with {i}/{pool_size}");
                return;
            }

            let packets = Packets::new(PACKETS_BATCH_SIZE, MAX_PACKET_BYTES);
            if let Err(e) = memory_return_tx.send(packets) {
                log::error!("failed to initialize memory pool: {e:?}");
                return;
            }
        }

        log::debug!("memory pool initialization complete");
    });
}
