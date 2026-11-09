#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_benchmark::{
    BenchmarkId, Criterion, bundle_project_root, criterion_group, criterion_main,
};
use luck_bundler::BundleOptions;
use luck_core::DynamicRequire;
use luck_token::LuaVersion;

fn bench_bundler(criterion: &mut Criterion) {
    let root = bundle_project_root();
    let entry_path = root.join("main.lua");
    let search_paths = vec!["?.lua".to_owned(), "?/init.lua".to_owned()];
    let bundle = || {
        luck_bundler::bundle(
            &entry_path,
            &BundleOptions {
                version: LuaVersion::Lua54,
                search_paths: &search_paths,
                project_root: &root,
                dynamic_require: DynamicRequire::default(),
            },
        )
    };
    let bundled = bundle();
    assert!(bundled.is_ok(), "{:?}", bundled.err());

    let mut group = criterion.benchmark_group("bundler");
    // Measures the full pipeline a `luck bundle` invocation runs:
    // resolution (with disk probes), graph construction, and emit over a
    // 40-module diamond DAG of generated code.
    group.bench_function(BenchmarkId::from_parameter("gen_modules"), |bencher| {
        bencher.iter(bundle);
    });
    group.finish();
}

criterion_group!(bundler, bench_bundler);
criterion_main!(bundler);
