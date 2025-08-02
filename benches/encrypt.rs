use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use valet::prelude::Key;

fn keygen(c: &mut Criterion) {
    c.bench_function("Key::new", |b| b.iter(|| Key::new()));
}

fn encryption(c: &mut Criterion) {
    let key = Key::new();
    c.bench_function("Key::encrypt", |b| {
        b.iter(|| key.encrypt(black_box(b"plaintext")))
    });
    let encrypted = key.encrypt(b"plaintext").expect("failed to encrypt");
    c.bench_function("Key::decrypt", |b| b.iter(|| key.decrypt(&encrypted)));
}

criterion_group!(benches, keygen, encryption);
criterion_main!(benches);
