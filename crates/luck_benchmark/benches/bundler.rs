use luck_allocator::ThreadAllocators;
use luck_benchmark::{BenchmarkId, Criterion, criterion_group, criterion_main};
use luck_benchmark_files::bundle_projects;
use luck_bundler::{BundleOptions, DynamicRequire};
use rayon::ThreadPoolBuilder;

fn bench_bundler(criterion: &mut Criterion) {
    // A pool of only the bench thread reads every module on it, so no work
    // waits on another thread or depends on the runner's core count.
    ThreadPoolBuilder::new()
        .num_threads(1)
        .use_current_thread()
        .build_global()
        .expect("nothing built the global pool before");

    let mut group = criterion.benchmark_group("bundler");
    let mut arenas = ThreadAllocators::new();
    for project in bundle_projects() {
        let options = BundleOptions {
            project_root: &project.root,
            dynamic_require: DynamicRequire::default(),
            luau_config: None,
        };
        let mut bundle = || {
            arenas.reset();
            luck_bundler::bundle(&arenas, &project.entry, &options)
        };
        bundle().unwrap_or_else(|errors| panic!("{} bundles: {errors:?}", project.name));
        // Measures the full pipeline a `luck bundle` invocation runs:
        // resolution (with disk probes), graph construction, and emit.
        group.bench_function(BenchmarkId::from_parameter(project.name), |bencher| {
            bencher.iter(&mut bundle);
        });
    }
    group.finish();
}

criterion_group!(bundler, bench_bundler);
criterion_main!(bundler);
