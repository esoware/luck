use luck_allocator::ThreadAllocators;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
use luck_benchmark_files::bundle_project_root;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_token::LuaVersion;
use rayon::ThreadPoolBuilder;

fn bench_bundler(criterion: &mut Criterion) {
    // A pool of only the bench thread reads every module on it, so no work
    // waits on another thread or depends on the runner's core count.
    ThreadPoolBuilder::new()
        .num_threads(1)
        .use_current_thread()
        .build_global()
        .expect("nothing built the global pool before");

    let root = bundle_project_root();
    let entry_path = root.join("main.lua");
    let search_paths = vec!["?.lua".to_owned(), "?/init.lua".to_owned()];
    let options = BundleOptions {
        version: LuaVersion::Lua54,
        search_paths: &search_paths,
        project_root: &root,
        dynamic_require: DynamicRequire::default(),
    };
    let mut arenas = ThreadAllocators::new();
    let mut bundle = || {
        arenas.reset();
        luck_bundler::bundle(&arenas, &entry_path, &options)
    };
    bundle().expect("the benchmark project bundles");

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
