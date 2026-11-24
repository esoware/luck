use core::num::NonZero;
use std::process::ExitCode;
use std::thread;

use clap::{Parser, Subcommand};

use crate::commands::{
    BuildArgs, BundleArgs, CheckArgs, FmtArgs, GraphArgs, InitArgs, LintArgs, LspArgs, MinifyArgs,
};
use crate::project::start_thread_pool;
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

    /// Hide progress, summaries, and warnings.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// How many threads to work on files with [default: one per CPU core].
    #[arg(long, global = true, value_name = "INT")]
    pub threads: Option<NonZero<usize>>,
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

impl Command {
    /// Whether the command works on its files on rayon's pool. The language
    /// server starts threads of its own.
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

pub(crate) fn run() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet { Verbosity::Quiet } else { Verbosity::Normal };
    let threads = cli
        .threads
        .unwrap_or_else(|| thread::available_parallelism().unwrap_or(NonZero::<usize>::MIN));
    if cli.command.uses_thread_pool()
        && let Err(exit) = start_thread_pool(threads)
    {
        return exit.into();
    }

    let outcome = match cli.command {
        Command::Init(args) => args.run(verbosity),
        Command::Build(args) => args.run(verbosity),
        Command::Bundle(args) => args.run(verbosity),
        Command::Minify(args) => args.run(),
        Command::Graph(args) => args.run(verbosity),
        Command::Lint(args) => args.run(verbosity),
        Command::Fmt(args) => args.run(verbosity),
        Command::Check(args) => args.run(verbosity),
        Command::Lsp(args) => args.run(threads),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => exit.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_is_a_count_of_one_or_more_any_command_takes() {
        let threads = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.threads);
        assert_eq!(threads(&["luck", "lint"]).ok(), Some(None));
        assert_eq!(threads(&["luck", "--threads", "4", "lint"]).ok(), Some(NonZero::new(4)));
        assert_eq!(threads(&["luck", "fmt", "--threads", "1"]).ok(), Some(NonZero::new(1)));
        assert_eq!(threads(&["luck", "check", "--threads", "0"]).ok(), None);
    }
}
