use std::cell::RefCell;
use std::io::Write as _;
use std::mem;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use clap::Args;
use luck_allocator::Allocator;
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver, to_slash_lossy};
use luck_semantic::SemanticBuilder;
use luck_stdlib::Variant;
use luck_token::{LineIndex, Span, index_to_offset, offset_to_index};
use luck_type_checker::{CancellationToken, CheckError, Checker, Config, Host};
use rustc_hash::FxHashMap;

use crate::exit::{Exit, fail_with_diags};
use crate::project::{ProjectFile, current_directory, project_files};
use crate::reporter::{DefaultReporter, Reporter as _, SourceFile, print_files};
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
    pub(crate) fn run(self, verbosity: Verbosity, threads: NonZero<usize>) -> Result<(), Exit> {
        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            None,
        )?;
        let type_errors = type_errors(&files, threads)?;

        let mut has_errors = !type_errors.luaurcs.is_empty();
        print_files(&type_errors.luaurcs);
        for_each_outcome_in_order(
            &files,
            |file, allocator| {
                let file_type_errors = type_errors.by_path.get(file.path.as_path());
                check_file(allocator, file, file_type_errors.map_or(&[], Vec::as_slice))
            },
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

/// What Luau's type checker found.
struct TypeErrors<'files> {
    /// By file, for each file with any.
    by_path: FxHashMap<&'files Path, Vec<Diagnostic>>,
    /// The E014 of each `.luaurc` Luau couldn't read.
    luaurcs: Vec<FileDiagnostics>,
}

/// What Luau's type checker finds in the `files` a standalone project
/// governs, from checking them together on `threads` threads. Roblox files
/// aren't checked until there are definitions of Roblox's API.
fn type_errors(files: &[ProjectFile], threads: NonZero<usize>) -> Result<TypeErrors<'_>, Exit> {
    let host = FileHost::new()?;
    let modules: Vec<(&Path, String)> = files
        .iter()
        .filter(|file| file.project.variant() == Variant::Standalone)
        .filter_map(|file| {
            let canonical = FileSystemOs.canonicalize(&file.path).ok()?;
            Some((file.path.as_path(), host.module_name(&canonical)))
        })
        .collect();
    if modules.is_empty() {
        return Ok(TypeErrors { by_path: FxHashMap::default(), luaurcs: Vec::new() });
    }

    let names: Vec<&str> = modules.iter().map(|(_, name)| name.as_str()).collect();
    let mut checker = Checker::new(host);
    let diags = match checker.check(&names, threads, &CancellationToken::new()) {
        Ok(diags) => diags,
        Err(CheckError::Internal { module, diag }) => {
            let path = checker.host_mut().path(&module);
            return Err(fail_with_diags(&[FileDiagnostics { path, diags: vec![diag] }]));
        }
        // Nothing cancels the token.
        Err(CheckError::Cancelled) => Vec::new(),
    };
    let by_path = modules
        .into_iter()
        .zip(diags)
        .filter(|(_, diags)| !diags.is_empty())
        .map(|((path, _), diags)| (path, diags))
        .collect();
    Ok(TypeErrors { by_path, luaurcs: mem::take(checker.host_mut().luaurc_errors.get_mut()) })
}

/// The file system as Luau's type checker reads it, with `require` resolved as
/// `luck build` resolves it, and each module named by its canonical path,
/// relative to the working directory when it's under it.
struct FileHost {
    working_directory: PathBuf,
    resolver: Resolver,
    /// Each directory's `.luaurc`, if it has one, as read the first time a
    /// module below it asked.
    luaurcs: RefCell<FxHashMap<PathBuf, Option<String>>>,
    /// The E014 of each `.luaurc` Luau couldn't read, from the first time it
    /// read it.
    luaurc_errors: RefCell<Vec<FileDiagnostics>>,
}

impl FileHost {
    fn new() -> Result<Self, Exit> {
        let working_directory = current_directory()?;
        Ok(Self {
            working_directory: FileSystemOs
                .canonicalize(&working_directory)
                .unwrap_or(working_directory),
            resolver: Resolver::new(),
            luaurcs: RefCell::default(),
            luaurc_errors: RefCell::default(),
        })
    }

    fn module_name(&self, canonical: &Path) -> String {
        let relative = canonical.strip_prefix(&self.working_directory).unwrap_or(canonical);
        to_slash_lossy(relative).into_owned()
    }

    fn path(&self, module: &str) -> PathBuf {
        self.working_directory.join(module)
    }
}

impl Host for FileHost {
    fn source(&self, module: &str) -> Option<Vec<u8>> {
        FileSystemOs.read(&self.path(module)).ok()
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let resolution = self.resolver.resolve(&self.path(requirer), path).ok()?;
        Some(self.module_name(&resolution.path))
    }

    fn config(&self, module: &str, config: &mut Config<'_>) {
        let path = self.path(module);
        let directories: Vec<&Path> = path.ancestors().skip(1).collect();
        let mut luaurcs = self.luaurcs.borrow_mut();
        for &directory in directories.iter().rev() {
            let mut is_first_read = false;
            let luaurc = luaurcs.entry(directory.to_path_buf()).or_insert_with(|| {
                is_first_read = true;
                let text = FileSystemOs.read(&directory.join(".luaurc")).ok()?;
                String::from_utf8(text).ok()
            });
            if let Some(text) = luaurc
                && let Err(error) = config.apply_luaurc(text)
                && is_first_read
            {
                let luaurc_path = directory.join(".luaurc");
                let relative = luaurc_path.strip_prefix(&self.working_directory);
                let diag = Diagnostic::e014(luaurc_error_span(&error, text), &error);
                self.luaurc_errors.borrow_mut().push(FileDiagnostics {
                    path: relative.map_or_else(|_| luaurc_path.clone(), Path::to_path_buf),
                    diags: vec![diag],
                });
            }
        }
    }
}

/// The line Luau's error for a `.luaurc` names, as `at line 3`, in `text`.
fn luaurc_error_span(error: &str, text: &str) -> Option<Span> {
    let (_, after) = error.split_once(" at line ")?;
    let line = after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let start = LineIndex::new(text.as_bytes()).offset(line, 1)?;
    let line_text = text.get(offset_to_index(start)..)?.lines().next().unwrap_or_default();
    Some(Span::new(start, start + index_to_offset(line_text.len())))
}

/// `file`'s syntax and compile errors, or else `type_errors`, rendered, or
/// `None` when it has neither.
fn check_file(
    allocator: &Allocator,
    file: &ProjectFile,
    type_errors: &[Diagnostic],
) -> Option<Vec<u8>> {
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
    let errors = if errors.is_empty() { type_errors.to_vec() } else { errors };
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

        let rendered = check_file(&Allocator::new(), &file, &[]).expect("an unread file fails");

        let text = String::from_utf8(rendered).expect("the report is UTF-8");
        let label = file.path.to_string_lossy();
        assert!(text.starts_with(&format!("[E010] error: cannot read file `{label}`: ")), "{text}");
        assert!(text.ends_with("help: check that the file exists and is readable\n"), "{text}");
    }
}
