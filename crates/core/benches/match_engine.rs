//! 5,000 个应用名和 10,000 条待办标题上的查询。
//!
//! 不按架构文档「性能测量」采样，结果不能当作 P95 已经达到。

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use lanwork_core::search::benchmark_corpus;

fn match_engine(c: &mut Criterion) {
    let index = benchmark_corpus(5_000, 10_000);
    let mut group = c.benchmark_group("match_engine");
    group.bench_function("prepare_15000", |bench| {
        bench.iter(|| black_box(benchmark_corpus(5_000, 10_000)));
    });
    group.bench_function("query_wx", |bench| {
        bench.iter(|| black_box(index.query("wx")));
    });
    group.bench_function("query_weixin", |bench| {
        bench.iter(|| black_box(index.query("weixin")));
    });
    group.bench_function("query_vsc", |bench| {
        bench.iter(|| black_box(index.query("vsc")));
    });
    group.bench_function("query_wei", |bench| {
        bench.iter(|| black_box(index.query("微")));
    });
    group.finish();
}

criterion_group!(benches, match_engine);
criterion_main!(benches);
