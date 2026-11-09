use clap::Args;
use luck_core::TransformConfig;

/// Declare both polarities of every pass from one list. The `overrides_with`
/// ids are generated from the field identifiers, so a flag can only ever
/// point at its own opposite.
macro_rules! transform_flags {
    ($($(#[doc = $documentation:expr])* $pass:ident / $no_pass:ident),+ $(,)?) => {
        /// Per-pass minifier toggles. Each pass has both polarities, so a flag
        /// can turn a pass on or off whatever `luck.json` says; the flag
        /// given last wins.
        #[derive(Args, Clone)]
        pub(crate) struct MinifyFlags {
            $(
                #[arg(long, overrides_with = stringify!($pass))]
                $no_pass: bool,
                $(#[doc = $documentation])*
                #[arg(long, overrides_with = stringify!($no_pass))]
                $pass: bool,
            )+
        }

        impl MinifyFlags {
            /// An explicit flag of either polarity wins over `config`, and
            /// `overrides_with` guarantees at most one of them is set.
            pub(crate) const fn apply_to(&self, mut config: TransformConfig) -> TransformConfig {
                $(config.$pass = (config.$pass || self.$pass) && !self.$no_pass;)+
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
    /// Rename globals the file defines (off by default). Only safe for a
    /// self-contained script, since anything that looks them up by their
    /// original names breaks.
    rename_globals / no_rename_globals,
}
