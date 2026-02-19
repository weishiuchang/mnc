// Single concern main.
// Make sure we manage the startup and shutdown of subordinate threads.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use clap::Parser;
use crossbeam_channel::{Receiver, Sender, bounded};
use regex::Regex;

use packet::{Packet, PacketBatch, PacketType};

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

// Some global variables to help control threads.
#[derive(Clone)]
pub struct SharedState {
    pub packet_count: Arc<AtomicU64>,
    pub should_exit: Arc<AtomicBool>,
    pub packet_type: PacketType,
    pub verbose: bool,
}

impl SharedState {
    fn new(packet_type: PacketType, verbose: bool) -> Self {
        Self {
            packet_count: Arc::new(AtomicU64::new(0)),
            should_exit: Arc::new(AtomicBool::new(false)),
            packet_type,
            verbose,
        }
    }

    // Meory guarantees
    pub fn add_count(&self, delta: u64) -> u64 {
        self.packet_count.fetch_add(delta, Ordering::Relaxed) + delta
    }
    pub fn get_count(&self) -> u64 {
        self.packet_count.load(Ordering::Relaxed)
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

    // Reader>Writer channel
    let (data_tx, data_rx): (Sender<PacketBatch>, Receiver<PacketBatch>) =
        bounded(args.buffer_size);

    // Reader>Statistics channel (optional)
    let (stats_tx, stats_rx) = if !args.quiet && (args.stats || args.verbose) {
        let (tx, rx) = bounded(args.buffer_size);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Reader pulls packets from network/file/stdin
    let reader_handle = reader::spawn(reader::ReaderConfig {
        input: args.input.clone(),
        iface: args.mgroup.0.clone(),
        mgroup: args.mgroup.1.clone(),
        port: args.port,
        data_tx,
        stats_tx,
        shared_state: shared_state.clone(),
        max_count,
    });
    all_threads.push(reader_handle);

    // Writer sends packets to network/file/stdout. Discards all packets by default.
    let writer_handle = writer::spawn(writer::WriterConfig {
        output: args.output.clone(),
        to_network: args.input.is_some(),
        iface: args.mgroup.0.clone(),
        mgroup: args.mgroup.1.clone(),
        port: args.port,
        ttl: args.ttl,
        data_rx,
        shared_state: shared_state.clone(),
        rate: args.rate,
    });
    all_threads.push(writer_handle);

    // Statistics gives us some useful information about the packets
    if let Some(stats_rx) = stats_rx {
        let statistics_handle = statistics::spawn(statistics::StatisticsConfig {
            data_rx: stats_rx,
            shared_state: shared_state.clone(),
        });
        all_threads.push(statistics_handle);
    }

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
                    .map_err(|e| error::LibError::Critical(format!("join error: {e:?}")))?
                    .inspect_err(|e| log::debug!("thread error: {e:?}"))?;
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
