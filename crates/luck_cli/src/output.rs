//! Output and diagnostic plumbing shared by the command handlers: writing
//! results to a file or stdout, reading stdin, and building the ariadne file
//! cache from a set of diagnostics.

use crate::render::{FileCache, render_diagnostics};
use crate::{EXIT_FAILURE, EXIT_USAGE};
use luck_core::diagnostics::Diagnostic;
use std::path::{Path, PathBuf};
use std::process;
use std::process::ExitCode;

pub(crate) fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Group size for the parallel drivers: a few batches per thread keeps
/// the pool saturated while bounding how many rendered outcomes are held
/// in memory at once.
pub(crate) fn parallel_chunk_size() -> usize {
    rayon::current_num_threads() * 4
}

pub(crate) fn current_dir_or_exit() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|error| {
        eprintln!("Error: failed to get current directory: {error}");
        process::exit(EXIT_USAGE as i32);
    })
}

pub(crate) fn write_output(output: Option<&str>, content: &str) {
    if let Some(path) = output {
        write_output_file(&PathBuf::from(path), content);
    } else {
        print!("{content}");
    }
}

pub(crate) fn write_output_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent()
        && !parent.exists()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("Error: failed to create output directory: {error}");
        process::exit(EXIT_FAILURE as i32);
    }
    if let Err(error) = std::fs::write(path, content) {
        eprintln!("Error: failed to write output: {error}");
        process::exit(EXIT_FAILURE as i32);
    }
}

/// Read the whole of stdin as UTF-8 text, mapping IO failure to an error
/// message plus `EXIT_FAILURE`.
pub(crate) fn read_stdin_source() -> Result<String, ExitCode> {
    use std::io::Read;
    let mut source = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut source) {
        eprintln!("Error: failed to read stdin: {error}");
        return Err(ExitCode::from(EXIT_FAILURE));
    }
    Ok(source)
}

/// Render the diagnostics to stderr and exit with `EXIT_FAILURE`. An optional
/// `(path, source)` pair seeds the cache for a document not readable from disk
/// (the minifier's in-memory bundle, stdin).
pub(crate) fn fail_with_diagnostics(errors: &[Diagnostic], source: Option<(&str, &str)>) -> ! {
    let mut cache = build_file_cache(errors);
    if let Some((path, src)) = source {
        cache.add_file(path.to_string(), src.to_string());
    }
    render_diagnostics(errors, &mut cache);
    process::exit(EXIT_FAILURE as i32);
}

pub(crate) fn build_file_cache(diagnostics: &[Diagnostic]) -> FileCache {
    let mut cache = FileCache::new();
    for diag in diagnostics {
        let os_path = diag.file_path.replace('/', std::path::MAIN_SEPARATOR_STR);
        if let Ok(source) = luck_core::source_io::read_source_file(&os_path) {
            cache.add_file(diag.file_path.clone(), source);
        }
    }
    cache
}
