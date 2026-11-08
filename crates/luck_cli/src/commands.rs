//! One module per subcommand. Each owns its `clap` argument struct, its `run`
//! handler, and its tests.

mod build;
mod bundle;
mod check;
mod fmt;
mod graph;
mod init;
mod lint;
mod lsp;
mod minify;

pub(crate) use build::BuildArgs;
pub(crate) use bundle::BundleArgs;
pub(crate) use check::CheckArgs;
pub(crate) use fmt::FmtArgs;
pub(crate) use graph::GraphArgs;
pub(crate) use init::InitArgs;
pub(crate) use lint::LintArgs;
pub(crate) use lsp::LspArgs;
pub(crate) use minify::MinifyArgs;
