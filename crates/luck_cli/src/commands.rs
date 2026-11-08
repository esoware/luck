//! One module per subcommand. Each owns its `clap` argument struct, its `run`
//! handler, and its tests.

pub(crate) mod build;
pub(crate) mod bundle;
pub(crate) mod check;
pub(crate) mod fmt;
pub(crate) mod graph;
pub(crate) mod init;
pub(crate) mod lint;
pub(crate) mod lsp;
pub(crate) mod minify;
