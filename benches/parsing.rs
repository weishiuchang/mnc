#![allow(clippy::unwrap_used)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use mnc::vita49;

// Original implementation, inlined for comparison
fn parse_header_original(packet: &[u8]) -> u16 {
    if packet.len() < 8 || packet.get(0..4) != Some(b"VRLP") {
        return 0;
    }
    let byte4 = packet.get(4).map(|&b| u16::from(b)).unwrap_or(0);
    let byte5 = packet.get(5).map(|&b| u16::from(b)).unwrap_or(0);
    (byte4 << 4) | (byte5 >> 4)
}

fn bench_vita49(c: &mut Criterion) {
    let packet = vec![b'V', b'R', b'L', b'P', 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut group = c.benchmark_group("vita49_parse_header");
    group.bench_function("original", |b| {
        b.iter(|| black_box(parse_header_original(black_box(&packet))));
    });
    group.bench_function("optimized", |b| {
        b.iter(|| black_box(vita49::parse_header(black_box(&packet)).frame_sequence_number));
    });
    group.finish();
}

criterion_group!(benches, bench_vita49);
criterion_main!(benches);
