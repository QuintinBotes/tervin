//! The hot path: scanning PTY output for escape sequences.
//!
//! Every byte a terminal displays passes through this. It is the one place in Tervin
//! where a constant factor is felt directly — a build log arriving at tens of megabytes
//! per second has to be scanned faster than it arrives, or output falls behind the
//! process producing it and the terminal feels laggy in a way no profile will localise.
//!
//! The numbers that matter are throughput on realistic output, not on a synthetic
//! worst case. Three shapes are measured because they stress different branches:
//! plain text (the common case), heavily coloured output (a compiler or a test runner),
//! and output dense with the markers Tervin actually cares about.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use terminal_core::OscScanner;

/// Plain build output: the overwhelming majority of what a terminal sees.
fn plain(lines: usize) -> Vec<u8> {
    (0..lines)
        .map(|i| format!("   Compiling some-crate v0.{i}.0 (/Users/dev/project/crates/thing)\n"))
        .collect::<String>()
        .into_bytes()
}

/// Colour-heavy output, as `cargo`, `jest`, and `pytest` all produce.
fn coloured(lines: usize) -> Vec<u8> {
    (0..lines)
        .map(|i| {
            format!(
                "\x1b[1m\x1b[31merror[E0{i:03}]\x1b[0m\x1b[1m: mismatched types\x1b[0m\n \
                 \x1b[1m\x1b[34m-->\x1b[0m src/lib.rs:{i}:9\n"
            )
        })
        .collect::<String>()
        .into_bytes()
}

/// Output dense with sequences Tervin acts on: prompt marks, cwd reports, and its own
/// command markers. A shell with integration active produces these on every prompt.
fn marker_dense(prompts: usize) -> Vec<u8> {
    (0..prompts)
        .map(|i| {
            format!(
                "\x1b]133;A\x07\x1b]7;file:///Users/dev/project\x07$ cargo test\n\
                 \x1b]133;C\x07running {i} tests\n\x1b]133;D;0\x07"
            )
        })
        .collect::<String>()
        .into_bytes()
}

fn scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("osc_scan");

    for (name, bytes) in [
        ("plain", plain(2_000)),
        ("coloured", coloured(1_000)),
        ("marker_dense", marker_dense(500)),
    ] {
        // Throughput rather than time, so the result is comparable across shapes and
        // states a number that can be checked against how fast output actually arrives.
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &bytes, |b, bytes| {
            b.iter(|| {
                let mut scanner = OscScanner::new();
                std::hint::black_box(scanner.feed_indexed(std::hint::black_box(bytes)))
            });
        });
    }

    group.finish();
}

/// The same bytes delivered in small reads.
///
/// A PTY does not hand over a megabyte at once, and a scanner that is fast on one large
/// slice can be slow across many small ones — the carry-over state between chunks is
/// what makes the difference, and it is also where the marker-splitting bug lived.
fn scan_chunked(c: &mut Criterion) {
    let bytes = coloured(1_000);
    let mut group = c.benchmark_group("osc_scan_chunked");
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    for chunk in [512usize, 4096, 65536] {
        group.bench_with_input(BenchmarkId::from_parameter(chunk), &chunk, |b, &chunk| {
            b.iter(|| {
                let mut scanner = OscScanner::new();
                for slice in bytes.chunks(chunk) {
                    std::hint::black_box(scanner.feed_indexed(slice));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, scan, scan_chunked);
criterion_main!(benches);
