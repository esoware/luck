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

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `output_path` can't
/// be written.
pub(crate) fn write_output(output_path: Option<&Path>, content: &str) -> Result<(), Exit> {
    if let Some(path) = output_path {
        return write_output_file(path, content);
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
        fs::create_dir_all(parent)
            .map_err(|e| operation_error(format!("failed to create output directory: {e}")))?;
    }
    fs::write(path, content).map_err(|e| operation_error(format!("failed to write output: {e}")))
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when stdin can't be
/// read.
pub(crate) fn read_stdin_source() -> Result<String, Exit> {
    io::read_to_string(io::stdin())
        .map_err(|e| operation_error(format!("failed to read stdin: {e}")))
}
