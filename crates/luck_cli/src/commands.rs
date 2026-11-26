mod build;
mod bundle;
mod check;
mod fmt;
mod graph;
mod init;
mod lint;
mod lsp;
mod minify;

use std::num::NonZero;

use clap::Subcommand;

use crate::exit::Exit;
use crate::thread_pool::start_thread_pool;
use crate::verbosity::Verbosity;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create luck.json and a src/main entry file in the current directory.
    Init(init::InitArgs),

    /// Build the project in luck.json: bundle, then minify if enabled.
    Build(build::BuildArgs),

    /// Bundle a multi-file project into a single file.
    Bundle(bundle::BundleArgs),

    /// Minify a source file.
    Minify(minify::MinifyArgs),

    /// Print the dependency graph of an entry file.
    Graph(graph::GraphArgs),

    /// Lint Luau source files.
    Lint(lint::LintArgs),

    /// Format Luau source files.
    Fmt(fmt::FmtArgs),

    /// Check Luau source files for syntax, compile, and type errors.
    Check(check::CheckArgs),

    /// Run the language server (LSP) over stdio, or TCP with --socket.
    Lsp(lsp::LspArgs),
}

impl Command {
    /// Starts rayon's pool with `threads` threads first when the command works
    /// on its files there. The language server starts `threads` workers of its
    /// own.
    pub(crate) fn run(self, verbosity: Verbosity, threads: NonZero<usize>) -> Result<(), Exit> {
        if self.uses_thread_pool() {
            start_thread_pool(threads)?;
        }
        match self {
            Self::Init(args) => args.run(verbosity),
            Self::Build(args) => args.run(verbosity),
            Self::Bundle(args) => args.run(verbosity),
            Self::Minify(args) => args.run(),
            Self::Graph(args) => args.run(verbosity),
            Self::Lint(args) => args.run(verbosity),
            Self::Fmt(args) => args.run(verbosity),
            Self::Check(args) => args.run(verbosity, threads),
            Self::Lsp(args) => args.run(threads),
        }
    }

    const fn uses_thread_pool(&self) -> bool {
        match self {
            Self::Build(_)
            | Self::Bundle(_)
            | Self::Graph(_)
            | Self::Lint(_)
            | Self::Fmt(_)
            | Self::Check(_) => true,
            Self::Init(_) | Self::Minify(_) | Self::Lsp(_) => false,
        }
    }
}
