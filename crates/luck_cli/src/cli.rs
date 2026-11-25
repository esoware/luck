use std::num::NonZero;
use std::process::ExitCode;
use std::thread;

use clap::Parser;

use crate::commands::Command;
use crate::verbosity::Verbosity;

#[derive(Parser)]
#[command(
    name = "luck",
    version,
    about = "Luau bundler, minifier, formatter, linter, and language server",
    long_about = "Luau bundler, minifier, formatter, linter, and language server.\nA bundle is a single file whose modules load lazily through a small inline loader, with no external runtime.",
    after_help = "Examples:\n  luck init\n  luck build\n  luck bundle src/main.luau -o dist/bundle.luau\n  luck minify input.luau -o output.luau\n  luck graph src/main.luau --format dot"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Hide progress, summaries, and warnings.
    #[arg(long, global = true)]
    quiet: bool,

    /// How many threads to work on files with [default: one per CPU core].
    #[arg(long, global = true, value_name = "INT")]
    threads: Option<NonZero<usize>>,
}

pub(crate) fn run() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet { Verbosity::Quiet } else { Verbosity::Normal };
    let threads = cli
        .threads
        .unwrap_or_else(|| thread::available_parallelism().unwrap_or(NonZero::<usize>::MIN));
    match cli.command.run(verbosity, threads) {
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
        assert_eq!(threads(&["luck", "lint"]).unwrap(), None);
        assert_eq!(threads(&["luck", "--threads", "4", "lint"]).unwrap(), NonZero::new(4));
        assert_eq!(threads(&["luck", "fmt", "--threads", "1"]).unwrap(), NonZero::new(1));
        assert!(threads(&["luck", "check", "--threads", "0"]).is_err());
    }
}
