//! Packet-pipeline micro-benchmarks.
//!
//! Run with: `cargo bench -p realmhound-core`
//!
//! These cover the CPU-bound decryption path that every captured game packet
//! passes through. Extend with additional benches (reassembly, parsing) as
//! hot paths are identified via the in-app profiler.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use realmhound_core::crypto::{RC4Cipher, INCOMING_KEY};

/// Deterministic pseudo-payload of the requested size.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

fn bench_rc4_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("rc4_apply");
    for &size in &[64usize, 1024, 8192] {
        let data = payload(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_function(format!("{size}B"), |b| {
            b.iter_batched(
                || (RC4Cipher::new(INCOMING_KEY), data.clone()),
                |(mut cipher, mut buf)| {
                    cipher.apply(black_box(&mut buf));
                    buf
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_rc4_new(c: &mut Criterion) {
    c.bench_function("rc4_new", |b| {
        b.iter(|| RC4Cipher::new(black_box(INCOMING_KEY)));
    });
}

criterion_group!(benches, bench_rc4_apply, bench_rc4_new);
criterion_main!(benches);
