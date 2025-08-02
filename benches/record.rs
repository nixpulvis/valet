use criterion::{Criterion, criterion_group, criterion_main};
use valet::prelude::*;

fn small_data(c: &mut Criterion) {
    let data = RecordData::plain("label", "secret");

    c.bench_function("Record::encode", |b| b.iter(|| data.encode()));
    let encoded = data.encode();
    c.bench_function("Record::decode", |b| {
        b.iter(|| RecordData::decode(&encoded))
    });

    c.bench_function("Record::compress", |b| b.iter(|| data.compress()));
    let compressed = data.compress().expect("failed to compress");
    c.bench_function("Record::decompress", |b| {
        b.iter(|| RecordData::decompress(&compressed).expect("failed to decompress"))
    });

    let lot = Lot::new("test");
    c.bench_function("Record::encrypt", |b| b.iter(|| data.encrypt(lot.key())));
    let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
    c.bench_function("Record::decrypt", |b| {
        b.iter(|| RecordData::decrypt(&encrypted, lot.key()))
    });
}

criterion_group!(all, small_data);
criterion_main!(all);
