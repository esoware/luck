//! The top-level `clap` model and dispatch to each command's handler.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::commands::{
    BuildArgs, BundleArgs, CheckArgs, FmtArgs, GraphArgs, InitArgs, LintArgs, LspArgs, MinifyArgs,
};
use crate::verbosity::Verbosity;

#[derive(Parser)]
#[command(
    name = "luck",
    version,
    about = "Lua/Luau bundler, minifier, formatter, linter, and language server",
    long_about = "Lua/Luau bundler, minifier, formatter, linter, and language server.\nA bundle is a single file whose modules load lazily through a small inline loader, with no external runtime.",
    after_help = "Examples:\n  luck init\n  luck build\n  luck bundle src/main.lua -t 54 -o dist/bundle.lua\n  luck minify input.luau -o output.luau\n  luck graph src/main.lua -t 54 --format dot"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Hide progress, summaries, and bundler warnings.
    #[arg(long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create luck.json and a src/main entry file in the current directory.
    Init(InitArgs),

    /// Build the project in luck.json: bundle, then minify if enabled.
    Build(BuildArgs),

    /// Bundle a multi-file project into a single file.
    Bundle(BundleArgs),

    /// Minify a source file.
    Minify(MinifyArgs),

    /// Print the dependency graph of an entry file.
    Graph(GraphArgs),

    /// Lint Lua/Luau source files.
    Lint(LintArgs),

    /// Format Lua/Luau source files.
    Fmt(FmtArgs),

    /// Check Lua/Luau source files for syntax and compile errors.
    Check(CheckArgs),

    /// Run the language server (LSP) over stdio, or TCP with --socket.
    Lsp(LspArgs),
}

/// Parses the process arguments, runs the chosen command, and returns its
/// exit code.
pub(crate) fn run() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet { Verbosity::Quiet } else { Verbosity::Normal };

    let outcome = match cli.command {
        Command::Init(args) => args.run(),
        Command::Build(args) => args.run(verbosity),
        Command::Bundle(args) => args.run(verbosity),
        Command::Minify(args) => args.run(),
        Command::Graph(args) => args.run(verbosity),
        Command::Lint(args) => args.run(verbosity),
        Command::Fmt(args) => args.run(verbosity),
        Command::Check(args) => args.run(verbosity),
        Command::Lsp(args) => args.run(),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => exit.into(),
    }
}
