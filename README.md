# mnc - Multcast Netcat

A high-performance CLI utility for sending and receiving multicast UDP packets, written in Rust. Think of it as `netcat` for multicast networking with support for specialized packet formats.

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.92.0+-orange.svg)](https://www.rust-lang.org)

## Features

- **High Performance**: Built with Rust for minimal overhead and maximum throughput
- **Multiple Packet Types**: Support for text, binary, VITA-49, and SDDS packet formats
- **Flexible I/O**: Read from stdin/file or multcast groups, write to stdout/file or multicast
- **Statistics**: Real-time packet statistics and verbose hex dump mode
- **Rate Limiting**: Control transmission rates for replay and simulation
- **Interface Selection**: Bind to specific network interfaces for multi-homed systems
- **Static Binary**: Single fully-static binary with no runtime dependencies

## Installation

### From Docker

```bash
docker pull weishiuchang/mnc:latest
docker run --rm weishiuchang/mnc:latest --help
```

### From Source

Requires Rust 1.92.0 or later:

```bash
git clone https://github.com/weishiuchang/mnc.git
cd mnc
cargo build --release
./target/release/mnc --help
```

### Pre-built Binaries

Download the latest release from the [Releases](../../releases) page.

## Usage

### Basic Examples

**Receive multicast packets:**
```bash
mnc 239.1.1.1
```

**Receive multicast on eth1 and port 5000:**
```bash
mnc eth1:239.1.1.1.1 -p 5000
```

**Send from stdin to multicast:**
```bash
echo "Hello, world!" | mnc 239.1.1.1 -i -
```

**Send multicast from file to default interface:**
```bash
mnc 239.1.1.1 -i ./input.bin -i binary
```

**Receive and save to file:**
```bash
mnc 239.1.1.1 -o ./output.bin
```

**Print packet counts every two seconds:**
```bash
mnc 239.1.1.1 -s
```

**Hex dump the first packet received:**
```bash
mnc 239.1.1.1 -v
```

### Packet Types

- **text** (default): Text-based packets
- **binary**: Raw binary data (no parsing done)
- **vita49**: VITA-49 radio transport protocol packets
- **sdds**: SDDS packets

## Use cases

### Network Testing
```bash
# Can we receive multicast on the default interface?
mnc 239.1.1.1 -s

# Send test multicast
echo "test" | mnc 239.1.1.1 -i -
```

### Data Distribution
```bash
# Broadcast file contents
mnc 239.1.1.1 -i ./data.bin -t binary

# Write multicast stream to file
mnc 239.1.1.1 -o ./data.bin
```

### Rate-Limited Replay
```bash
# Send with rate limiting
mnc 239.1.1.1 -i ./input.bin -r 1000
```

## Protocol Support

### VITA-49
The VITA Radio Transport (VRT) protocol for radio signal metadata and data transport. mnc recognizes the VRLP (VITA-49 Link Protocol) frame format.

### SDDS
Signal Data Distribution System format used for signal distribution with timing information.

## Architecture

mnc uses a multi-threaded architecture with bounded channels for efficient packet processing:

- **Reader Thread**: Receives multicast packets or reads from input
- **Writer Thread**: Sends to multicast or writes to output
- **Statistics Thread**: Collects and displays real-time statistics

## Building

### Release Build
```bash
cargo build --release
```

### Static Binary (Alpine Linux)
```bash
cargo build --release --target x86_64-unknown-linux-musl
```

### Docker Build
```bash
# Release (scratch)
docker build -t mnc:0.9.0 .

# Debug (busybox)
docker build --target debug -t mnc:0.9.0-debug .
```

## Development

### Prerequisites
- Rust 1.92.0 or later

### Code Quality
```bash
# Run tests
cargo test

# Lint with Clippy
cargo clippy --fix

# Format code
cargo fmt
```

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Make your changes with tests
4. Ensure `cargo clippy` passes with no warnings
5. Format with `cargo fmt`
6. Squash your branch for ease of review
7. Ensure single concern PRs
8. Absolutely no unsafe code!
9. Submit a pull request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgements

- Build with [Rust](https://www.rust-lang.org)
- Inspired by the classic `netcat` utility

## Support

For bugs and feature requests, please open an issue on GitHub.
