use criterion::{criterion_group, criterion_main, Criterion};

fn benchmark_simple(c: &mut Criterion) {
    c.bench_function("simple_loop", |b| {
        b.iter(|| {
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            std::hint::black_box(sum);
        })
    });
}

criterion_group!(benches, benchmark_simple);
criterion_main!(benches);
