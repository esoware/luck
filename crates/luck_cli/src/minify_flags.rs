//! The per-pass transform toggles shared by `bundle` and `minify`, and
//! their projection onto a [`TransformConfig`].

use clap::Args;
use luck_core::TransformConfig;

/// Declare both polarities of every pass from one list. The `overrides_with`
/// ids are generated from the field identifiers, so a flag can only ever
/// point at its own opposite.
macro_rules! transform_flags {
    ($($(#[doc = $doc:expr])* $pass:ident / $no_pass:ident),+ $(,)?) => {
        /// CLI flags that toggle individual minifier transforms. Every pass
        /// carries both polarities, so a flag can override whatever
        /// `luck.json` configured instead of only narrowing it; the flag
        /// given last wins.
        #[derive(Args, Clone)]
        pub(crate) struct MinifyFlags {
            $(
                #[arg(long, overrides_with = stringify!($pass))]
                $no_pass: bool,
                $(#[doc = $doc])*
                #[arg(long, overrides_with = stringify!($no_pass))]
                $pass: bool,
            )+
        }

        impl MinifyFlags {
            pub(crate) fn apply_to(&self, mut config: TransformConfig) -> TransformConfig {
                $(config.$pass = resolve(config.$pass, self.$pass, self.$no_pass);)+
                config
            }
        }
    };
}

transform_flags! {
    remove_dead_code / no_remove_dead_code,
    simplify_statements / no_simplify_statements,
    fold_constants / no_fold_constants,
    inline_locals / no_inline_locals,
    merge_locals / no_merge_locals,
    simplify_indexes / no_simplify_indexes,
    shorten_strings / no_shorten_strings,
    shorten_numbers / no_shorten_numbers,
    simplify_parens / no_simplify_parens,
    rename_locals / no_rename_locals,
    lift_locals / no_lift_locals,
    /// Rename globals defined in this file (breaks cross-chunk consumers
    /// that expect the original _G keys; off unless the script is fully
    /// self-contained).
    rename_globals / no_rename_globals,
}

/// Project one pass onto its configured value: an explicit flag of either
/// polarity wins, and `overrides_with` guarantees at most one is set.
fn resolve(is_configured: bool, should_enable: bool, should_disable: bool) -> bool {
    (is_configured || should_enable) && !should_disable
}
