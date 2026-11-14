#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::path::Path;

use luck_ast::{Block, Statement, Transform};
use luck_bundler::{BlockBundle, BundleOptions, BundleResult, bundle, bundle_block};
use luck_core::Diagnostic;
use luck_token::{CommentKind, LuaVersion, leading_hot_comments};

/// Bundles `entry` as text and as a tree, which has to be the tree the text
/// parses to, and returns the text's result.
///
/// # Errors
///
/// Returns the diagnostics that stopped the bundle, which the tree reports
/// alike.
#[expect(clippy::panic_in_result_fn, reason = "a form that disagrees fails the calling test")]
pub(crate) fn bundle_both(
    entry: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let text_result = bundle(entry, options);
    match (&text_result, bundle_block(entry, options)) {
        (Ok(text), Ok(tree)) => assert_tree_is_the_parse(&text.output, tree, options.version),
        (Err(text_errors), Err(tree_errors)) => assert_eq!(
            format!("{text_errors:?}"),
            format!("{tree_errors:?}"),
            "the two forms report different errors"
        ),
        (text, tree) => panic!("only one form bundles:\ntext: {text:?}\ntree: {tree:?}"),
    }
    text_result
}

/// The parse holds a `;` for each one the text writes, which the tree,
/// whose printer writes one where it's needed, leaves out.
struct WithoutEmptyStatements;

impl Transform for WithoutEmptyStatements {
    fn transform_block(&mut self, mut block: Block) -> Block {
        block.stmts.retain(|stmt| !matches!(stmt, Statement::EmptyStatement(_)));
        self.walk_block(block)
    }
}

fn assert_tree_is_the_parse(output: &str, tree: BlockBundle, version: LuaVersion) {
    let parsed = luck_parser::parse(output, version);
    assert!(parsed.errors.is_empty(), "the bundle must parse: {:?}\n{output}", parsed.errors);
    let expected = WithoutEmptyStatements.transform_block(parsed.block);
    let actual = WithoutEmptyStatements.transform_block(tree.block);
    assert_eq!(
        luck_codegen::compact(&actual).expect("the tree prints"),
        luck_codegen::compact(&expected).expect("the parse prints"),
        "the tree prints apart from the bundle's parse:\n{output}"
    );
    // What the minifier keeps on top of the text it reads.
    let mut text_header = String::new();
    if let Some(shebang) =
        parsed.comments.first().filter(|comment| comment.kind == CommentKind::Shebang)
    {
        text_header.push_str(shebang.span.text(output).trim_end_matches('\r'));
        text_header.push('\n');
    }
    if version.is_luau() {
        for comment in leading_hot_comments(&parsed.comments, output) {
            text_header.push_str(comment.span.text(output).trim_end());
            text_header.push('\n');
        }
    }
    assert_eq!(tree.header, text_header, "the tree's header isn't the bundle's:\n{output}");
}
