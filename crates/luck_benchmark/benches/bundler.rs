#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use core::slice;

use luck_allocator::Allocator;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
use luck_benchmark_files::bundle_project_root;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_token::LuaVersion;

fn bench_bundler(criterion: &mut Criterion) {
    let root = bundle_project_root();
    let entry_path = root.join("main.lua");
    let search_paths = vec!["?.lua".to_owned(), "?/init.lua".to_owned()];
    let options = BundleOptions {
        version: LuaVersion::Lua54,
        search_paths: &search_paths,
        project_root: &root,
        dynamic_require: DynamicRequire::default(),
    };
    // One arena keeps discovery on the bench thread, so no work waits on
    // another thread or depends on the runner's core count.
    let mut allocator = Allocator::new();
    let mut bundle = || {
        allocator.reset();
        luck_bundler::bundle(slice::from_mut(&mut allocator), &entry_path, &options)
    };
    let bundled = bundle();
    assert!(bundled.is_ok(), "{:?}", bundled.err());

    let mut group = criterion.benchmark_group("bundler");
    // Measures the full pipeline a `luck bundle` invocation runs:
    // resolution (with disk probes), graph construction, and emit over a
    // 40-module diamond DAG of generated code.
    group.bench_function(BenchmarkId::from_parameter("gen_modules"), |bencher| {
        bencher.iter(&mut bundle);
    });
    group.finish();
}

criterion_group!(bundler, bench_bundler);
criterion_main!(bundler);
