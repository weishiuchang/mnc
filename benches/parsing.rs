#![allow(clippy::unwrap_used)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use mnc::{sdds, vita49};

fn vita49_packet() -> Vec<u8> {
    vec![
        b'V', b'R', b'L', b'P', 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0, 0, 0, 0, 0,
    ]
}

fn sdds_packet() -> Vec<u8> {
    let mut pkt = vec![0u8; 1080];
    pkt[0] = 0b10110101;
    pkt[1] = 0b11010111;
    pkt[2] = 0x12;
    pkt[3] = 0x34;
    pkt[8..16].copy_from_slice(&0x0123_4567_89AB_CDEFu64.to_be_bytes());
    pkt
}

fn bench_vita49(c: &mut Criterion) {
    let pkt = vita49_packet();
    c.bench_function("vita49_parse_header", |b| {
        b.iter(|| black_box(vita49::parse_header(black_box(&pkt))));
    });
}

fn bench_sdds(c: &mut Criterion) {
    let pkt = sdds_packet();
    let mut g = c.benchmark_group("sdds");

    // Before: two separate calls (what statistics.rs used to do)
    g.bench_function("separate_calls", |b| {
        b.iter(|| {
            let p = black_box(&pkt);
            black_box((sdds::frame_sequence_number(p), sdds::time_tag(p)))
        });
    });

    // After: single bounds check + one 128-bit load
    g.bench_function("parse_frame_header", |b| {
        b.iter(|| {
            let h = sdds::parse_frame_header(black_box(&pkt));
            black_box((h.frame_sequence_number, h.time_tag))
        });
    });

    g.finish();
}

criterion_group!(benches, bench_vita49, bench_sdds);
criterion_main!(benches);
