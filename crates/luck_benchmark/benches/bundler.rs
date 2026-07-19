use luck_benchmark::corpus::bundle_project_root;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
use luck_core::types::LuaTarget;

fn bench_bundler(criterion: &mut Criterion) {
    let root = bundle_project_root();
    let entry_path = root.join("main.lua");
    let search_paths = vec!["?.lua".to_string(), "?/init.lua".to_string()];

    let mut group = criterion.benchmark_group("bundler");
    // Measures the full pipeline a `luck bundle` invocation runs:
    // resolution (with disk probes), graph construction, and emit over a
    // 40-module diamond DAG of generated code.
    group.bench_function(BenchmarkId::from_parameter("gen_modules"), |b| {
        b.iter(|| {
            luck_bundler::bundle(&entry_path, LuaTarget::Lua54, &search_paths, &root)
                .expect("bench corpus must bundle")
        });
    });
    group.finish();
}

criterion_group!(bundler, bench_bundler);
criterion_main!(bundler);
