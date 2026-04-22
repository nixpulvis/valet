use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use valet::encrypt::Key;

fn keygen(c: &mut Criterion) {
    c.bench_function("Key::new", |b| b.iter(Key::<()>::generate));
}

fn encryption(c: &mut Criterion) {
    let key = Key::<()>::generate();
    let encrypt = || key.encrypt(black_box(b"plaintext"));
    c.bench_function("Key::encrypt", |b| b.iter(encrypt));
    let encrypted = encrypt().expect("failed to encrypt");
    c.bench_function("Key::decrypt", |b| b.iter(|| key.decrypt(&encrypted)));

    let encrypt_aad = || key.encrypt_with_aad(black_box(b"plaintext"), black_box(b"aad"));
    c.bench_function("Key::encrypt_with_aad", |b| b.iter(encrypt_aad));
    let encrypted_aad = encrypt_aad().expect("failed to encrypt");
    c.bench_function("Key::decrypt_with_aad", |b| {
        b.iter(|| key.decrypt_with_aad(&encrypted_aad, black_box(b"aad")))
    });
}

criterion_group!(benches, keygen, encryption);
criterion_main!(benches);
