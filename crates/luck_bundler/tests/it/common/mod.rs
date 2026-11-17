#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use core::iter;
use std::path::Path;

use luck_allocator::Allocator;
use luck_bundler::{BundleOptions, BundleResult, bundle, bundle_minified};
use luck_core::Diagnostic;
use luck_minifier::TransformConfig;

/// `count` fresh arenas, one for each thread a bundle may read modules on.
pub(crate) fn arenas(count: usize) -> Vec<Allocator> {
    iter::repeat_with(Allocator::new).take(count).collect()
}

/// Bundles `entry` as text on one thread and on several, which have to agree,
/// and minified, which has to be what minifying the text gives, and returns
/// the text's result.
///
/// # Errors
///
/// Returns the diagnostics that stopped the bundle, which every form reports
/// alike.
#[expect(clippy::panic_in_result_fn, reason = "a form that disagrees fails the calling test")]
pub(crate) fn bundle_both(
    entry: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let transforms = TransformConfig::default();
    let text_result = bundle(&mut arenas(1), entry, options);
    let threaded_result = bundle(&mut arenas(4), entry, options);
    assert_eq!(
        format!("{text_result:?}"),
        format!("{threaded_result:?}"),
        "bundling on threads changes the bundle"
    );
    match (&text_result, bundle_minified(&mut arenas(3), entry, options, &transforms)) {
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
