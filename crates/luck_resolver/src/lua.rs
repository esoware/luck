use std::path::Path;

use luck_core::Diagnostic;

use crate::path::normalize_path;
use crate::{ResolveRequest, ResolvedModule};

/// # Errors
///
/// Returns E004, listing the tried paths, when no template names a file.
pub(crate) fn resolve(
    search_paths: &[String],
    project_root: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let dotted_path = request.module.replace('.', "/");
    // A leading dot must not turn a relative template into a rooted path.
    let module_path = dotted_path.trim_start_matches('/');
    let mut tried_paths = Vec::new();

    for template in search_paths {
        let relative_path = template.replace('?', module_path);
        let candidate = project_root.join(&relative_path);

        if candidate.is_file() {
            return Ok(ResolvedModule { path: normalize_path(&candidate), warnings: Vec::new() });
        }

        tried_paths.push(relative_path);
    }

    Err(Diagnostic::e004(request.from_file, request.span, request.module, &tried_paths))
}
