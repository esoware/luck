//! The top-level `clap` argument model and dispatch. Each variant carries its
//! command's own argument struct (defined in that command's module); `run`
//! parses the process arguments and hands off to the matching handler.

use crate::Verbosity;
use crate::commands::{build, bundle, check, fmt, graph, init, lint, lsp, minify};
use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "luck",
    version,
    about = "Zero-runtime Lua/Luau bundler",
    long_about = "Bundles multi-file Lua/Luau projects into a single output file.\nModules load lazily through a tiny inline loader - no external runtime required.",
    after_help = "Examples:\n  luck init\n  luck build\n  luck bundle src/main.lua -t 54 -o dist/bundle.lua\n  luck minify input.luau -o output.luau\n  luck graph src/main.lua -t 54 --format dot"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Initialize a new project with luck.json
    Init(init::InitArgs),

    /// Build project using luck.json config
    Build(build::BuildArgs),

    /// Bundle a multi-file project into a single file
    Bundle(bundle::BundleArgs),

    /// Minify a source file
    Minify(minify::MinifyArgs),

    /// Show the dependency graph
    Graph(graph::GraphArgs),

    /// Lint Lua/Luau source files (config-driven, oxlint-style)
    Lint(lint::LintArgs),

    /// Format Lua/Luau source files (config-driven, oxfmt-style)
    Fmt(fmt::FmtArgs),

    /// Parse and check Lua/Luau source files for errors (config-driven)
    Check(check::CheckArgs),

    /// Run the language server (LSP) over stdio, or TCP with --socket.
    Lsp(lsp::LspArgs),
}

/// Parses CLI arguments and dispatches to the appropriate subcommand,
/// returning the process exit code per the documented convention.
pub fn run() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else {
        Verbosity::Normal
    };

    match cli.command {
        Command::Init(args) => args.run(),
        Command::Build(args) => args.run(verbosity),
        Command::Bundle(args) => args.run(verbosity),
        Command::Minify(args) => args.run(verbosity),
        Command::Graph(args) => args.run(verbosity),
        Command::Lint(args) => args.run(verbosity),
        Command::Fmt(args) => args.run(verbosity),
        Command::Check(args) => args.run(verbosity),
        Command::Lsp(args) => args.run(),
    }
}
