//! The `--no-<pass>` transform toggles shared by `bundle` and `minify`, and
//! their projection onto a [`TransformConfig`].

use clap::Args;
use luck_core::TransformConfig;

/// CLI flags that disable individual minifier transforms.
#[derive(Args, Clone)]
pub(crate) struct MinifyFlags {
    #[arg(long)]
    no_remove_dead_code: bool,
    #[arg(long)]
    no_simplify_statements: bool,
    #[arg(long)]
    no_fold_constants: bool,
    #[arg(long)]
    no_inline_locals: bool,
    #[arg(long)]
    no_merge_locals: bool,
    #[arg(long)]
    no_simplify_indexes: bool,
    #[arg(long)]
    no_shorten_strings: bool,
    #[arg(long)]
    no_shorten_numbers: bool,
    #[arg(long)]
    no_simplify_parens: bool,
    #[arg(long)]
    no_rename_locals: bool,
    #[arg(long)]
    no_lift_locals: bool,
    /// Rename globals defined in this file (breaks cross-chunk consumers
    /// that expect the original _G keys; off unless the script is fully
    /// self-contained).
    #[arg(long)]
    rename_globals: bool,
}

impl MinifyFlags {
    pub(crate) fn to_transform_config(&self) -> TransformConfig {
        TransformConfig {
            remove_dead_code: !self.no_remove_dead_code,
            simplify_statements: !self.no_simplify_statements,
            fold_constants: !self.no_fold_constants,
            inline_locals: !self.no_inline_locals,
            merge_locals: !self.no_merge_locals,
            simplify_indexes: !self.no_simplify_indexes,
            shorten_strings: !self.no_shorten_strings,
            shorten_numbers: !self.no_shorten_numbers,
            simplify_parens: !self.no_simplify_parens,
            rename_locals: !self.no_rename_locals,
            lift_locals: !self.no_lift_locals,
            rename_globals: self.rename_globals,
        }
    }
}
