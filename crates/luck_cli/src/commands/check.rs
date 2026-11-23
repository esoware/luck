use std::io::{self, BufWriter, Write as _};
use std::path::PathBuf;

use clap::Args;
use luck_allocator::Allocator;
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_semantic::SemanticBuilder;

use crate::exit::Exit;
use crate::output::OUTPUT_BUFFER_BYTES;
use crate::project::{ProjectFile, for_each_outcome_in_order, project_files};
use crate::reporter::{DefaultReporter, Reporter as _, SourceFile};
use crate::verbosity::Verbosity;

#[derive(Args)]
pub(crate) struct CheckArgs {
    /// Files, directories, or globs to check, and `!` globs to leave out
    /// [default: current directory].
    paths: Vec<String>,

    /// Use this config for every file [default: the nearest luck.json above
    /// each file].
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Exit 0 when a glob, or the whole run, matches no file.
    #[arg(long)]
    no_error_on_unmatched_pattern: bool,
}

impl CheckArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Failure`] if any file can't be read or has errors, and
    /// [`Exit::Usage`] on a bad config or path, or when no file matches.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            None,
        )?;

        let mut has_errors = false;
        {
            // One lock and one flush for the whole run.
            let mut stderr = BufWriter::with_capacity(OUTPUT_BUFFER_BYTES, io::stderr().lock());
            for_each_outcome_in_order(
                &files,
                |file, allocator| check_file(allocator, file),
                |outcome| {
                    if let Some(rendered) = outcome {
                        has_errors = true;
                        let _unused = stderr.write_all(&rendered);
                    }
                    Ok(())
                },
            )
        }?;

        if has_errors {
            return Err(Exit::Failure);
        }

        if verbosity != Verbosity::Quiet {
            eprintln!("ok: {} file(s) checked", files.len());
        }

        Ok(())
    }
}

fn check_file(allocator: &Allocator, file: &ProjectFile) -> Option<Vec<u8>> {
    let file_path = file.path.as_path();
    let source = match FileSystemOs.read(file_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Some(
                format!("error: cannot read `{}`: {error}\n", file_path.display()).into_bytes(),
            );
        }
    };

    let version = file.project.target_for(Some(file_path)).version();
    let result = luck_parser::parse(allocator, &source, version);
    let errors = if result.errors.is_empty() {
        // The compile-time errors real Lua reports beyond the grammar, such as
        // const writes and unresolved gotos.
        SemanticBuilder::new()
            .with_counts(result.counts)
            .with_check(true)
            .build(&result.chunk)
            .diagnostics
    } else {
        result.errors
    };
    if errors.is_empty() {
        return None;
    }

    let mut rendered = Vec::new();
    let label = file_path.to_string_lossy();
    let source_file = SourceFile { label: &label, text: Some(&source), version };
    DefaultReporter.render(source_file, &errors, &mut rendered);
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser as _;

    use super::CheckArgs;
    use crate::cli::{Cli, Command};

    #[test]
    fn check_is_top_level_command_with_paths_and_config() {
        let cli = Cli::try_parse_from(["luck", "check", "src", "main.lua", "-c", "luck.json"])
            .expect("check parses");
        let Command::Check(CheckArgs { paths, config, no_error_on_unmatched_pattern }) =
            cli.command
        else {
            panic!("expected Command::Check");
        };
        assert_eq!(paths, vec!["src".to_owned(), "main.lua".to_owned()]);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
        assert!(!no_error_on_unmatched_pattern);
    }
}
