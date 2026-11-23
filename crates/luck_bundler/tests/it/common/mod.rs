#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::path::Path;

use luck_allocator::{Allocator, ThreadAllocators};
use luck_ast::MIN_STACK_BYTES;
use luck_bundler::{BundleOptions, BundleResult, bundle, bundle_minified};
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_minifier::TransformConfig;
use luck_resolver::to_slash_lossy;
use rayon::ThreadPoolBuilder;

/// What `work` returns on a pool of `thread_count` threads, each with the
/// stack every stage needs, which it runs on as a caller inside a pool does.
pub(crate) fn on_pool<T, Work>(thread_count: usize, work: Work) -> T
where
    T: Send,
    Work: FnOnce() -> T + Send,
{
    ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .stack_size(MIN_STACK_BYTES)
        .build()
        .expect("the pool starts")
        .install(work)
}

/// What `work` returns on this thread, outside any pool.
///
/// The bundler then reads modules on rayon's global pool, which this builds
/// first with the stack every stage needs, since every test here that
/// reaches that pool comes through this.
pub(crate) fn off_pool<T>(work: impl FnOnce() -> T) -> T {
    let _already_built = ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build_global();
    work()
}

/// Bundles `entry` as text on one thread, and from outside the pool on many,
/// which have to agree, and minified on a few, which has to be what minifying
/// the text gives, and returns the text's result.
///
/// # Errors
///
/// Returns the diagnostics that stopped the bundle, which every form reports
/// alike.
#[expect(clippy::panic_in_result_fn, reason = "a form that disagrees fails the calling test")]
pub(crate) fn bundle_both(
    entry: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<FileDiagnostics>> {
    let transforms = TransformConfig::default();
    let text_result = on_pool(1, || bundle(&ThreadAllocators::new(), entry, options));
    let threaded_result = off_pool(|| bundle(&ThreadAllocators::new(), entry, options));
    assert_eq!(
        format!("{text_result:?}"),
        format!("{threaded_result:?}"),
        "bundling on threads changes the bundle"
    );
    let minified_result =
        on_pool(3, || bundle_minified(&ThreadAllocators::new(), entry, options, &transforms));
    match (&text_result, minified_result) {
        (Ok(text), Ok(minified)) => {
            let output = &text.output;
            let allocator = Allocator::new();
            let expected = luck_minifier::minify(&allocator, output, options.version, &transforms)
                .unwrap_or_else(|errors| panic!("the bundle must parse: {errors:?}\n{output}"));
            assert_eq!(
                minified.output, expected,
                "the tree minifies apart from the text:\n{output}"
            );
        }
        (Err(text_errors), Err(minified_errors)) => assert_eq!(
            format!("{text_errors:?}"),
            format!("{minified_errors:?}"),
            "the two forms report different errors"
        ),
        (text, minified) => {
            panic!("only one form bundles:\ntext: {text:?}\nminified: {minified:?}")
        }
    }
    text_result
}

/// Each diagnostic in `files`, with the slash form of its file's path.
pub(crate) fn located(files: &[FileDiagnostics]) -> Vec<(String, &Diagnostic)> {
    files
        .iter()
        .flat_map(|file| {
            let path = to_slash_lossy(&file.path).into_owned();
            file.diags.iter().map(move |diag| (path.clone(), diag))
        })
        .collect()
}

/// Each diagnostic's code, in order.
pub(crate) fn codes(files: &[FileDiagnostics]) -> Vec<&str> {
    files.iter().flat_map(|file| &file.diags).map(|diag| diag.code).collect()
}
