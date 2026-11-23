use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use crate::exit::{Exit, operation_error};

/// What a path command buffers before writing to stdout or stderr. One file's
/// rendered diagnostics often outgrow the default buffer, which would write
/// each through on its own.
pub(crate) const OUTPUT_BUFFER_BYTES: usize = 1 << 18;

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

/// `content` is bytes, as a Lua source is, which reach stdout or the file
/// as they are.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `output_path` or
/// stdout can't be written.
pub(crate) fn write_output(output_path: Option<&Path>, content: &[u8]) -> Result<(), Exit> {
    if let Some(path) = output_path {
        return write_output_file(path, content);
    }
    io::stdout()
        .lock()
        .write_all(content)
        .map_err(|e| operation_error(format!("failed to write output: {e}")))
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when `path` or its
/// parent directory can't be written.
pub(crate) fn write_output_file(path: &Path, content: &[u8]) -> Result<(), Exit> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|e| operation_error(format!("failed to create output directory: {e}")))?;
    }
    fs::write(path, content).map_err(|e| operation_error(format!("failed to write output: {e}")))
}

/// Bytes, as Lua reads a source.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when stdin can't be
/// read.
pub(crate) fn read_stdin_source() -> Result<Vec<u8>, Exit> {
    let mut source = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut source)
        .map_err(|e| operation_error(format!("failed to read stdin: {e}")))?;
    Ok(source)
}
