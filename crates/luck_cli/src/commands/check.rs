use std::io::Write as _;
use std::path::PathBuf;

use clap::Args;
use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_semantic::SemanticBuilder;

use crate::exit::Exit;
use crate::project::{ProjectFile, project_files};
use crate::reporter::{DefaultReporter, Reporter as _, SourceFile};
use crate::thread_pool::for_each_outcome_in_order;
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
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            None,
        )?;

        let mut has_errors = false;
        for_each_outcome_in_order(
            &files,
            |file, allocator| check_file(allocator, file),
            |outcome, output| {
                if let Some(rendered) = outcome {
                    has_errors = true;
                    let _ = output.stderr.write_all(&rendered);
                }
                Ok(())
            },
        )?;

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
    let label = file_path.to_string_lossy();
    let source = match FileSystemOs.read(file_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let mut rendered = Vec::new();
            let source_file = SourceFile { label: &label, text: None };
            DefaultReporter.render(source_file, &[Diagnostic::e010(&label, &error)], &mut rendered);
            return Some(rendered);
        }
    };

    let result = luck_parser::parse(allocator, &source);
    let errors = if result.errors.is_empty() {
        // The compile-time errors Luau reports beyond the grammar, such as an
        // assignment to a `const`.
        SemanticBuilder::new()
            .with_counts(result.counts)
            .with_check(true)
            .build(&result.chunk)
            .errors
    } else {
        result.errors
    };
    if errors.is_empty() {
        return None;
    }

    let mut rendered = Vec::new();
    let source_file = SourceFile { label: &label, text: Some(&source) };
    DefaultReporter.render(source_file, &errors, &mut rendered);
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use clap::Parser as _;
    use luck_allocator::Allocator;
    use luck_config::Project;

    use super::{CheckArgs, check_file};
    use crate::cli::Cli;
    use crate::commands::Command;
    use crate::project::ProjectFile;

    #[test]
    fn check_is_top_level_command_with_paths_and_config() {
        let cli = Cli::try_parse_from(["luck", "check", "src", "main.luau", "-c", "luck.json"])
            .expect("check parses");
        let Command::Check(CheckArgs { paths, config, no_error_on_unmatched_pattern }) =
            cli.command
        else {
            panic!("expected Command::Check");
        };
        assert_eq!(paths, vec!["src".to_owned(), "main.luau".to_owned()]);
        assert_eq!(config, Some(PathBuf::from("luck.json")));
        assert!(!no_error_on_unmatched_pattern);
    }

    #[test]
    fn an_unreadable_file_reports_e010() {
        let directory = tempfile::tempdir().expect("tempdir");
        let project = Arc::new(Project::defaults(directory.path().to_path_buf()));
        let file = ProjectFile { path: directory.path().join("gone.luau"), project };

        let rendered = check_file(&Allocator::new(), &file).expect("an unread file fails");

        let text = String::from_utf8(rendered).expect("the report is UTF-8");
        let label = file.path.to_string_lossy();
        assert!(text.starts_with(&format!("[E010] error: cannot read file `{label}`: ")), "{text}");
        assert!(text.ends_with("help: check that the file exists and is readable\n"), "{text}");
    }
}
