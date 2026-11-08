//! Helpers shared by the command handlers: output and stdin I/O, and size
//! display.

use std::path::Path;
use std::{fs, io};

use crate::exit::{Exit, operation_error};

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
