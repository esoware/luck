use std::borrow::Cow;
use std::cell::RefCell;
use std::io::Write as _;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use clap::Args;
use luck_allocator::Allocator;
use luck_config::DefinitionSource;
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver, is_definition_file, to_slash_lossy};
use luck_semantic::SemanticBuilder;
use luck_stdlib::{SecurityLevel, roblox_definitions};
use luck_token::{LineIndex, Span, index_to_offset, offset_to_index};
use luck_type_checker::{
    CancellationToken, CheckError, Checker, Config, Host, SavedConfig, luau_config_aliases,
};
use rustc_hash::FxHashMap;

use crate::exit::{Exit, fail_with_diags};
use crate::project::{ProjectFile, current_directory, project_files};
use crate::reporter::{DefaultReporter, Reporter as _, SourceFile, print_document, print_files};
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

        let mut has_errors = !type_errors.configs.is_empty();
        print_files(&type_errors.configs);
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
    /// The E014 of each `.luaurc` and `.config.luau` Luau couldn't read.
    configs: Vec<FileDiagnostics>,
}

/// The security level and definitions of a project, which decide what its
/// files are checked with.
type Definitions<'files> = (SecurityLevel, &'files [DefinitionSource]);

/// What Luau's type checker finds in `files`, from checking together, on
/// `threads` threads, the files of the projects with one security level and
/// one list of definitions, which need one checker. A definition file is no
/// module, so only loading it checks it.
fn type_errors(files: &[ProjectFile], threads: NonZero<usize>) -> Result<TypeErrors<'_>, Exit> {
    let mut groups: Vec<(Definitions<'_>, Vec<&ProjectFile>)> = Vec::new();
    for file in files {
        let settings = (file.project.security(), file.project.definitions());
        match groups.iter_mut().find(|(group_settings, _)| *group_settings == settings) {
            Some((_, group)) => group.push(file),
            None => groups.push((settings, vec![file])),
        }
    }

    let host = FileHost::new()?;
    let mut by_path = FxHashMap::default();
    for ((security, definitions), group) in groups {
        let names_and_texts: Vec<(String, Cow<'_, str>)> = definitions
            .iter()
            .map(|source| match source {
                DefinitionSource::Roblox => ("@roblox".to_owned(), roblox_definitions(security)),
                DefinitionSource::File { path, text } => {
                    (host.module_name(path), Cow::Borrowed(text.as_str()))
                }
            })
            .collect();
        let modules: Vec<(&Path, String)> = group
            .into_iter()
            .filter(|file| !is_definition_file(&file.path))
            .filter_map(|file| {
                let canonical = FileSystemOs.canonicalize(&file.path).ok()?;
                Some((file.path.as_path(), host.module_name(&canonical)))
            })
            .collect();
        let names: Vec<&str> = modules.iter().map(|(_, name)| name.as_str()).collect();
        let mut checker = Checker::new(&host);
        for (name, text) in &names_and_texts {
            if let Err(diags) = checker.load_definitions(name, text) {
                print_document(name, text.as_bytes(), &diags);
                return Err(Exit::Failure);
            }
        }
        if definitions.contains(&DefinitionSource::Roblox) {
            // What Roblox Studio types past its definitions: the class a
            // string names, and the `Enum.Material` its definitions call
            // `EnumMaterial`.
            checker.return_named_type("Instance", "new");
            checker.return_named_type("ServiceProvider", "GetService");
            checker.return_named_type("ServiceProvider", "FindService");
            checker.bind_type_namespace("Enum", "EnumItem");
        }
        let diags = match checker.check(&names, threads, &CancellationToken::new()) {
            Ok(diags) => diags,
            Err(CheckError::Internal { module, diag }) => {
                let path = host.path(&module);
                return Err(fail_with_diags(&[FileDiagnostics { path, diags: vec![diag] }]));
            }
            // Nothing cancels the token.
            Err(CheckError::Cancelled) => Vec::new(),
        };
        by_path.extend(
            modules
                .into_iter()
                .zip(diags)
                .filter(|(_, diags)| !diags.is_empty())
                .map(|((path, _), diags)| (path, diags)),
        );
    }
    Ok(TypeErrors { by_path, configs: host.config_errors.into_inner() })
}

/// The file system as Luau's type checker reads it, with `require` resolved as
/// `luck build` resolves it, and each module named by its canonical path,
/// relative to the working directory when it's under it. Every group's
/// checker reads through one, so a run reads each config once.
struct FileHost {
    working_directory: PathBuf,
    resolver: Resolver,
    /// The config each directory gives the modules in it, from its `.luaurc`
    /// or `.config.luau` and the ones above it, read the first time a module
    /// below it asks, as `luau-analyze` keeps them.
    configs: RefCell<FxHashMap<PathBuf, SavedConfig>>,
    /// The E014 of each config Luau couldn't read.
    config_errors: RefCell<Vec<FileDiagnostics>>,
}

impl FileHost {
    fn new() -> Result<Self, Exit> {
        let working_directory = current_directory()?;
        Ok(Self {
            working_directory: FileSystemOs
                .canonicalize(&working_directory)
                .unwrap_or(working_directory),
            resolver: Resolver::new().with_luau_config(luau_config_aliases),
            configs: RefCell::default(),
            config_errors: RefCell::default(),
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

impl Host for &FileHost {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        FileSystemOs.read(&self.path(module)).ok().map(Cow::Owned)
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let resolution = self.resolver.resolve(&self.path(requirer), path).ok()?;
        Some(self.module_name(&resolution.path))
    }

    /// As luau-analyze reads them: a directory with both a `.luaurc` and a
    /// `.config.luau` applies neither.
    fn config(&self, module: &str, config: &mut Config<'_>) {
        let path = self.path(module);
        let directories: Vec<&Path> = path.ancestors().skip(1).collect();
        let mut configs = self.configs.borrow_mut();
        let cached = directories.iter().position(|directory| configs.contains_key(*directory));
        if let Some(index) = cached {
            config.set(&configs[directories[index]]);
        }
        for &directory in directories[..cached.unwrap_or(directories.len())].iter().rev() {
            let read =
                |name: &str| String::from_utf8(FileSystemOs.read(&directory.join(name)).ok()?).ok();
            let (luaurc, luau_config) = (read(".luaurc"), read(".config.luau"));
            let applied = match (&luaurc, &luau_config) {
                (Some(text), Some(_)) => Some((
                    ".luaurc",
                    text,
                    Err("Both .luaurc and .config.luau files exist".to_owned()),
                )),
                (Some(text), None) => Some((".luaurc", text, config.apply_luaurc(text))),
                (None, Some(source)) => {
                    Some((".config.luau", source, config.apply_luau_config(source)))
                }
                (None, None) => None,
            };
            if let Some((name, text, Err(error))) = applied {
                let config_path = directory.join(name);
                let relative = config_path.strip_prefix(&self.working_directory);
                let diag = Diagnostic::e014(config_error_span(&error, text), &error);
                self.config_errors.borrow_mut().push(FileDiagnostics {
                    path: relative.map_or_else(|_| config_path.clone(), Path::to_path_buf),
                    diags: vec![diag],
                });
            }
            if let Some(saved) = config.saved() {
                configs.insert(directory.to_path_buf(), saved);
            }
        }
    }
}

/// The line Luau's error for a config names in `text`, as `at line 3` for a
/// `.luaurc` and `config:3:` for a `.config.luau`.
fn config_error_span(error: &str, text: &str) -> Option<Span> {
    let after = error.split_once(" at line ").map(|(_, after)| after);
    let after = after.or_else(|| error.strip_prefix("config:"))?;
    let line = after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let start = LineIndex::new(text.as_bytes()).offset(line, 1)?;
    let line_text = text.get(offset_to_index(start)..)?.lines().next().unwrap_or_default();
    Some(Span::new(start, start + index_to_offset(line_text.len())))
}

/// `file`'s syntax and compile errors, or else `type_errors`, rendered, or
/// `None` when it has neither. A definition file has syntax errors alone.
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

    if is_definition_file(file_path) {
        let errors = luck_parser::parse_definitions(allocator, &source).errors;
        return (!errors.is_empty()).then(|| render(&label, &source, &errors));
    }
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
    (!errors.is_empty()).then(|| render(&label, &source, &errors))
}

fn render(label: &str, source: &[u8], errors: &[Diagnostic]) -> Vec<u8> {
    let mut rendered = Vec::new();
    DefaultReporter.render(SourceFile { label, text: Some(source) }, errors, &mut rendered);
    rendered
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use clap::Parser as _;
    use luck_allocator::Allocator;
    use luck_config::Project;
    use luck_resolver::FileSystemOs;

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
        let project = Arc::new(Project::defaults(directory.path().to_path_buf(), FileSystemOs));
        let file = ProjectFile { path: directory.path().join("gone.luau"), project };

        let rendered = check_file(&Allocator::new(), &file, &[]).expect("an unread file fails");

        let text = String::from_utf8(rendered).expect("the report is UTF-8");
        let label = file.path.to_string_lossy();
        assert!(text.starts_with(&format!("[E010] error: cannot read file `{label}`: ")), "{text}");
        assert!(text.ends_with("help: check that the file exists and is readable\n"), "{text}");
    }
}
