//! Fuzzy matching, which runs on every keystroke of the file picker.
//!
//! The budget here is a frame: a user typing at speed expects the list to update between
//! keystrokes, so ranking a whole repository has to finish in well under 16 ms. The
//! matcher is a dynamic program over (query × candidate), so cost grows with both —
//! which makes the interesting measurement a realistic repository size against queries
//! of the length people actually type.
//!
//! A greedy matcher would be far cheaper and was tried first. It ranked `sm` →
//! `session_manager` below unrelated files, because sliding matched positions rightwards
//! *maximises* spread rather than minimising it. The DP is the cost of correct ranking,
//! and this benchmark is what keeps that cost honest.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use file_index::fuzzy::Matcher;

/// Paths shaped like a real Rust and TypeScript repository.
fn corpus(count: usize) -> Vec<String> {
    let dirs = [
        "crates/agent-runtime/src",
        "crates/terminal-core/src",
        "crates/block-engine/src",
        "ui/src/components",
        "ui/src/lib",
        "docs",
        "node_modules/@types/react",
    ];
    let names = [
        "mod",
        "lib",
        "normalize",
        "protocol",
        "session",
        "index",
        "store",
    ];
    (0..count)
        .map(|i| {
            format!(
                "{}/{}{}.{}",
                dirs[i % dirs.len()],
                names[i % names.len()],
                i,
                if i % 3 == 0 { "rs" } else { "tsx" }
            )
        })
        .collect()
}

fn rank(c: &mut Criterion) {
    let mut group = c.benchmark_group("fuzzy_rank");

    // 20k files is a large monorepo; 2k is an ordinary project. Both are measured
    // because the shape of the answer differs — at 2k the per-call overhead dominates,
    // at 20k the inner loop does.
    for count in [2_000usize, 20_000] {
        let paths = corpus(count);
        group.throughput(Throughput::Elements(count as u64));

        // Queries of the length people actually type. A single character is the
        // pathological case: it matches nearly everything, so every candidate runs the
        // full inner loop.
        for query in ["s", "sm", "acpnorm", "uicomp"] {
            group.bench_with_input(
                BenchmarkId::new(format!("{count}"), query),
                &query,
                |b, query| {
                    // One matcher for the pass, which is how `rank` uses it. Creating
                    // one per candidate would measure allocation, not matching.
                    b.iter(|| {
                        let mut matcher = Matcher::new();
                        let mut total = 0i64;
                        for path in &paths {
                            if let Some(m) = matcher.score(std::hint::black_box(query), path) {
                                total += m.score as i64;
                            }
                        }
                        std::hint::black_box(total)
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, rank);
criterion_main!(benches);
