use std::borrow::Cow;
use std::cell::RefCell;
use std::io::{self, Write as _};
use std::num::NonZero;
use std::path::{Path, PathBuf};

use clap::Args;
use luck_allocator::{Allocator, AllocatorPool};
use luck_config::DefinitionSource;
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver, is_definition_file, to_slash_lossy};
use luck_semantic::SemanticBuilder;
use luck_stdlib::{SecurityLevel, roblox_definitions};
use luck_type_checker::{
    CancellationToken, CheckError, Checker, Config, DirectoryConfigs, Host, luau_config_aliases,
};
use rayon::iter::{
    IndexedParallelIterator as _, IntoParallelRefIterator as _, ParallelIterator as _,
};
use rustc_hash::FxHashMap;

use crate::exit::{Exit, fail_with_diags};
use crate::project::{ProjectFile, current_directory, project_files};
use crate::reporter::{DefaultReporter, Reporter as _, SourceFile, print_files};
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
    /// Reads every file once, then checks types on this thread while rayon's
    /// pool parses, since Luau reads the whole module graph on one thread
    /// before it checks on several.
    pub(crate) fn run(self, verbosity: Verbosity, threads: NonZero<usize>) -> Result<(), Exit> {
        let files = project_files(
            self.config.as_deref(),
            &self.paths,
            self.no_error_on_unmatched_pattern,
            None,
        )?;
        let sources: Vec<io::Result<Vec<u8>>> =
            files.par_iter().map(|file| FileSystemOs.read(&file.path)).collect();
        let pool = AllocatorPool::new(rayon::current_num_threads());
        let mut compiled = Vec::new();
        let typed = rayon::in_place_scope(|scope| {
            scope.spawn(|_| {
                compiled = files
                    .par_iter()
                    .zip(&sources)
                    .map(|(file, source)| match source {
                        Ok(source) => compile_errors(&pool.get(), file, source),
                        Err(_) => Vec::new(),
                    })
                    .collect();
            });
            type_errors(&files, &sources, threads)
        })?;

        let mut has_errors = !typed.elsewhere.is_empty();
        print_files(&typed.elsewhere);
        let reports: Vec<Option<Vec<u8>>> = files
            .par_iter()
            .zip(&sources)
            .zip(compiled)
            .zip(typed.by_file)
            .map(|(((file, source), compile_errors), type_errors)| {
                let errors = if compile_errors.is_empty() { type_errors } else { compile_errors };
                report(file, source, &errors)
            })
            .collect();
        let mut stderr = io::stderr().lock();
        for rendered in reports.iter().flatten() {
            has_errors = true;
            // A failed write has nowhere to be reported.
            let _ = stderr.write_all(rendered);
        }

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
struct TypeErrors {
    /// Each file's, in order, a definition file's from loading it.
    by_file: Vec<Vec<Diagnostic>>,
    /// In files the run doesn't check: the errors of each definition file
    /// Luau couldn't load, and the E014 of each `.luaurc` and `.config.luau`
    /// it couldn't read.
    elsewhere: Vec<FileDiagnostics>,
}

/// The security level and definitions of a project, which decide what its
/// files are checked with.
type Definitions<'files> = (SecurityLevel, &'files [DefinitionSource]);

/// What Luau's type checker finds in `files`, whose bytes are `sources`, from
/// checking together, on `threads` threads, the files of the projects with
/// one security level and one list of definitions, which need one checker. A
/// definition file is no module, so only loading it checks it, and one Luau
/// can't load adds nothing. Its errors are reported once, on the file when the
/// run checks it, however many groups load it.
fn type_errors(
    files: &[ProjectFile],
    sources: &[io::Result<Vec<u8>>],
    threads: NonZero<usize>,
) -> Result<TypeErrors, Exit> {
    let mut host = FileHost::new()?;
    let modules: Vec<Option<String>> = files
        .iter()
        .map(|file| Some(host.module_name(&FileSystemOs.canonicalize(&file.path).ok()?)))
        .collect();
    host.sources = modules
        .iter()
        .zip(sources)
        .filter_map(|(module, source)| Some((module.clone()?, source.as_deref().ok()?)))
        .collect();
    let mut groups: Vec<(Definitions<'_>, Vec<usize>)> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let settings = (file.project.security(), file.project.definitions());
        match groups.iter_mut().find(|(group_settings, _)| *group_settings == settings) {
            Some((_, group)) => group.push(index),
            None => groups.push((settings, vec![index])),
        }
    }
    let definition_files: FxHashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .filter(|&(index, _)| is_definition_file(&files[index].path))
        .filter_map(|(index, module)| Some((module.as_deref()?, index)))
        .collect();

    let mut by_file = vec![Vec::new(); files.len()];
    let mut elsewhere: Vec<FileDiagnostics> = Vec::new();
    for ((security, definitions), group) in groups {
        let names_and_texts: Vec<(String, &str)> = definitions
            .iter()
            .map(|source| match source {
                DefinitionSource::Roblox => ("@roblox".to_owned(), roblox_definitions(security)),
                DefinitionSource::File { path, text } => (host.module_name(path), text.as_str()),
            })
            .collect();
        let group_modules: Vec<(usize, &str)> = group
            .into_iter()
            .filter(|&index| !is_definition_file(&files[index].path))
            .filter_map(|index| Some((index, modules[index].as_deref()?)))
            .collect();
        let names: Vec<&str> = group_modules.iter().map(|&(_, name)| name).collect();
        let mut checker = Checker::new(&host);
        for (name, text) in &names_and_texts {
            let Err(diags) = checker.load_definitions(name, text) else {
                continue;
            };
            match definition_files.get(name.as_str()) {
                Some(&index) if by_file[index].is_empty() => by_file[index] = diags,
                None if elsewhere.iter().all(|file| file.path != Path::new(name)) => {
                    elsewhere.push(FileDiagnostics { path: PathBuf::from(name), diags });
                }
                Some(_) | None => {}
            }
        }
        if definitions.contains(&DefinitionSource::Roblox) {
            checker.type_as_roblox_studio();
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
        for (&(index, _), module_diags) in group_modules.iter().zip(diags) {
            by_file[index] = module_diags;
        }
    }
    elsewhere.extend(host.config_errors.into_inner());
    Ok(TypeErrors { by_file, elsewhere })
}

/// The file system as Luau's type checker reads it, with `require` resolved as
/// `luck build` resolves it, and each module named by its canonical path,
/// relative to the working directory when it's under it. Every group's
/// checker reads through one, so a run reads each config once.
struct FileHost<'files> {
    working_directory: PathBuf,
    resolver: Resolver,
    configs: DirectoryConfigs,
    /// The source of each file the run checks, by its module, which Luau
    /// reads from here rather than the disk.
    sources: FxHashMap<String, &'files [u8]>,
    /// The E014 of each config Luau couldn't read.
    config_errors: RefCell<Vec<FileDiagnostics>>,
}

impl FileHost<'_> {
    fn new() -> Result<Self, Exit> {
        let working_directory = current_directory()?;
        Ok(Self {
            working_directory: FileSystemOs
                .canonicalize(&working_directory)
                .unwrap_or(working_directory),
            resolver: Resolver::new().with_luau_config(luau_config_aliases),
            configs: DirectoryConfigs::default(),
            sources: FxHashMap::default(),
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

impl Host for &FileHost<'_> {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        match self.sources.get(module) {
            Some(source) => Some(Cow::Borrowed(source)),
            None => FileSystemOs.read(&self.path(module)).ok().map(Cow::Owned),
        }
    }

    fn resolve(&self, requirer: &str, path: &str) -> Option<String> {
        let resolution = self.resolver.resolve(&self.path(requirer), path).ok()?;
        Some(self.module_name(&resolution.path))
    }

    fn config(&self, module: &str, config: &mut Config<'_>) {
        let read = |path: &Path| String::from_utf8(FileSystemOs.read(path).ok()?).ok();
        let errors = self.configs.apply(&self.path(module), config, read);
        self.config_errors.borrow_mut().extend(errors.into_iter().map(|(config_path, diag)| {
            let relative = config_path.strip_prefix(&self.working_directory);
            FileDiagnostics {
                path: relative.map_or_else(|_| config_path.clone(), Path::to_path_buf),
                diags: vec![diag],
            }
        }));
    }
}

/// The syntax errors of `file`, whose bytes are `source`, or else its compile
/// errors, which a definition file has none of.
fn compile_errors(allocator: &Allocator, file: &ProjectFile, source: &[u8]) -> Vec<Diagnostic> {
    if is_definition_file(&file.path) {
        return luck_parser::parse_definitions(allocator, source).errors;
    }
    let result = luck_parser::parse(allocator, source);
    if !result.errors.is_empty() {
        return result.errors;
    }
    // The compile-time errors Luau reports beyond the grammar, such as an
    // assignment to a `const`.
    SemanticBuilder::new().with_counts(result.counts).with_check(true).build(&result.chunk).errors
}

/// `errors`, all in `file`, rendered over `source`, or `None` when there are
/// none, or else an E010 when `source` couldn't be read.
fn report(
    file: &ProjectFile,
    source: &io::Result<Vec<u8>>,
    errors: &[Diagnostic],
) -> Option<Vec<u8>> {
    let label = file.path.to_string_lossy();
    let mut rendered = Vec::new();
    match source {
        Err(error) => {
            let source_file = SourceFile { label: &label, text: None };
            DefaultReporter.render(source_file, &[Diagnostic::e010(&label, error)], &mut rendered);
        }
        Ok(_) if errors.is_empty() => return None,
        Ok(source) => {
            let source_file = SourceFile { label: &label, text: Some(source) };
            DefaultReporter.render(source_file, errors, &mut rendered);
        }
    }
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use clap::Parser as _;
    use luck_config::Project;
    use luck_resolver::{FileSystem as _, FileSystemOs};

    use super::{CheckArgs, report};
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

        let rendered =
            report(&file, &FileSystemOs.read(&file.path), &[]).expect("an unread file fails");

        let text = String::from_utf8(rendered).expect("the report is UTF-8");
        let label = file.path.to_string_lossy();
        assert!(text.starts_with(&format!("[E010] error: cannot read file `{label}`: ")), "{text}");
        assert!(text.ends_with("help: check that the file exists and is readable\n"), "{text}");
    }
}
