#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::path::Path;

use luck_bundler::{BundleOptions, BundleResult, bundle, bundle_minified};
use luck_core::Diagnostic;
use luck_minifier::TransformConfig;

/// Bundles `entry` as text and minified, which has to be what minifying the
/// text gives, and returns the text's result.
///
/// # Errors
///
/// Returns the diagnostics that stopped the bundle, which the minified form
/// reports alike.
#[expect(clippy::panic_in_result_fn, reason = "a form that disagrees fails the calling test")]
pub(crate) fn bundle_both(
    entry: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let transforms = TransformConfig::default();
    let text_result = bundle(entry, options);
    match (&text_result, bundle_minified(entry, options, &transforms)) {
        (Ok(text), Ok(minified)) => {
            let output = &text.output;
            let expected = luck_minifier::minify(output, options.version, &transforms)
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
