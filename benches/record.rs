use criterion::{Criterion, criterion_group, criterion_main};
use valet::{prelude::*, record::Label, uuid::Uuid};

fn small_data(c: &mut Criterion) {
    let data = Data::new(
        Label::Simple("label".to_string()),
        "secret".try_into().unwrap(),
    ).expect("password validation failed");

    let encode = || data.encode();
    c.bench_function("Record::encode", |b| b.iter(encode));
    let encoded = encode();
    c.bench_function("Record::decode", |b| b.iter(|| Data::decode(&encoded)));

    let compress = || data.compress();
    c.bench_function("Record::compress", |b| b.iter(compress));
    let compressed = compress().expect("failed to compress");
    c.bench_function("Record::decompress", |b| {
        b.iter(|| Data::decompress(&compressed).expect("failed to decompress"))
    });

    let lot = Lot::new("test");
    let encrypt = || data.encrypt(lot.key());
    c.bench_function("Record::encrypt", |b| b.iter(encrypt));
    let encrypted = encrypt().expect("failed to encrypt");
    c.bench_function("Record::decrypt", |b| {
        b.iter(|| Data::decrypt(&encrypted, lot.key()))
    });

    let aad = Uuid::<Record>::now().to_string() + &Uuid::<Record>::now().to_string();
    let encrypt_aad = || data.encrypt_with_aad(lot.key(), aad.as_bytes());
    c.bench_function("Record::encrypt_with_aad", |b| b.iter(encrypt_aad));
    let encrypted_aad = encrypt_aad().expect("failed to encrypt");
    c.bench_function("Record::decrypt_with_aad", |b| {
        b.iter(|| Data::decrypt_with_aad(&encrypted_aad, lot.key(), aad.as_bytes()))
    });
}

criterion_group!(all, small_data);
criterion_main!(all);
