use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use crate::exit::{Exit, operation_error};

#[expect(clippy::cast_precision_loss, reason = "f64 rounds only a size past 2^53 bytes")]
pub(crate) fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// `content` is bytes, as a Luau source is, which reach stdout or the file
/// as they are.
pub(crate) fn write_output(output_path: Option<&Path>, content: &[u8]) -> Result<(), Exit> {
    if let Some(path) = output_path {
        return write_output_file(path, content);
    }
    io::stdout()
        .lock()
        .write_all(content)
        .map_err(|e| operation_error(format!("failed to write output: {e}")))
}

pub(crate) fn write_output_file(path: &Path, content: &[u8]) -> Result<(), Exit> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|e| operation_error(format!("failed to create output directory: {e}")))?;
    }
    fs::write(path, content).map_err(|e| operation_error(format!("failed to write output: {e}")))
}

/// Bytes, as Luau reads a source.
pub(crate) fn read_stdin_source() -> Result<Vec<u8>, Exit> {
    let mut source = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut source)
        .map_err(|e| operation_error(format!("failed to read stdin: {e}")))?;
    Ok(source)
}
