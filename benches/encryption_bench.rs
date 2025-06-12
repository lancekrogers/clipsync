//! Benchmarks for encryption performance

use clipsync::history::encryption::Encryptor;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

fn bench_encrypt_decrypt(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let encryptor = rt.block_on(async { Encryptor::new().await.unwrap() });

    let mut group = c.benchmark_group("encryption");

    // Benchmark different payload sizes
    for size in &[1024, 10 * 1024, 100 * 1024, 1024 * 1024, 5 * 1024 * 1024] {
        let data = vec![b'A'; *size];

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_function(format!("encrypt_{}KB", size / 1024), |b| {
            b.iter(|| {
                let _encrypted = encryptor.encrypt(black_box(&data)).unwrap();
            });
        });

        let encrypted = encryptor.encrypt(&data).unwrap();
        group.bench_function(format!("decrypt_{}KB", size / 1024), |b| {
            b.iter(|| {
                let _decrypted = encryptor.decrypt(black_box(&encrypted)).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_checksum(c: &mut Criterion) {
    let mut group = c.benchmark_group("checksum");

    for size in &[1024, 100 * 1024, 1024 * 1024, 5 * 1024 * 1024] {
        let data = vec![b'B'; *size];

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_function(format!("sha256_{}KB", size / 1024), |b| {
            b.iter(|| {
                let _checksum = Encryptor::compute_checksum(black_box(&data));
            });
        });
    }

    group.finish();
}

fn bench_compression(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let encryptor = rt.block_on(async { Encryptor::new().await.unwrap() });

    let mut group = c.benchmark_group("compression");

    // Highly compressible data (repeated pattern)
    let compressible_1mb = vec![b'A'; 1024 * 1024];

    // Less compressible data (random-like)
    let mut less_compressible_1mb = Vec::with_capacity(1024 * 1024);
    for i in 0..1024 * 1024 {
        less_compressible_1mb.push((i % 256) as u8);
    }

    group.throughput(Throughput::Bytes(1024 * 1024));
    group.bench_function("encrypt_compressible_1MB", |b| {
        b.iter(|| {
            let _encrypted = encryptor.encrypt(black_box(&compressible_1mb)).unwrap();
        });
    });

    group.bench_function("encrypt_less_compressible_1MB", |b| {
        b.iter(|| {
            let _encrypted = encryptor
                .encrypt(black_box(&less_compressible_1mb))
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_encrypt_decrypt,
    bench_checksum,
    bench_compression
);
criterion_main!(benches);
