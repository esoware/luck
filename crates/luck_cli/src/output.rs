//! Helpers shared by the command handlers: error reporting, output and stdin
//! I/O, and size display.

use core::fmt::Display;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use luck_core::Diagnostic;

use crate::exit::Exit;
use crate::render::{print_diags, print_document_diags};

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::float_arithmetic,
    reason = "display math, and its exact rounding is part of the output"
)]
pub(crate) fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn report_error(message: impl Display, exit: Exit) -> Exit {
    eprintln!("Error: {message}");
    exit
}

/// Report a bad argument or config and return [`Exit::Usage`].
pub(crate) fn usage_error(message: impl Display) -> Exit {
    report_error(message, Exit::Usage)
}

/// Report an operation that ran and failed, and return [`Exit::Failure`].
pub(crate) fn operation_error(message: impl Display) -> Exit {
    report_error(message, Exit::Failure)
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working
/// directory can't be read.
pub(crate) fn current_directory() -> Result<PathBuf, Exit> {
    env::current_dir()
        .map_err(|error| usage_error(format!("failed to get current directory: {error}")))
}

/// Writes `content` to the `output_path` file, or to stdout without one.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `output_path` can't
/// be written.
pub(crate) fn write_output(output_path: Option<&str>, content: &str) -> Result<(), Exit> {
    if let Some(path) = output_path {
        return write_output_file(Path::new(path), content);
    }
    print!("{content}");
    Ok(())
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `path` or its
/// parent directory can't be written.
pub(crate) fn write_output_file(path: &Path, content: &str) -> Result<(), Exit> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|error| {
            operation_error(format!("failed to create output directory: {error}"))
        })?;
    }
    fs::write(path, content)
        .map_err(|error| operation_error(format!("failed to write output: {error}")))
}

/// Read the whole of stdin as UTF-8 text.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when stdin can't be
/// read.
pub(crate) fn read_stdin_source() -> Result<String, Exit> {
    io::read_to_string(io::stdin())
        .map_err(|error| operation_error(format!("failed to read stdin: {error}")))
}

/// Render the diagnostics to stderr and return [`Exit::Failure`].
///
/// `document` is a `(path, text)` document that isn't on disk, such as a
/// bundle being minified or stdin, that every diagnostic points into. Without
/// one, the files the diagnostics point into are read from disk.
pub(crate) fn fail_with_diags(diags: &[Diagnostic], document: Option<(String, String)>) -> Exit {
    match document {
        Some((path, source)) => print_document_diags(path, source, diags),
        None => print_diags(diags),
    }
    Exit::Failure
}
