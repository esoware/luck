use std::path::Path;

use luck_core::{Diagnostic, normalize_path};

use crate::{ResolveRequest, ResolvedModule};

/// Tries each search path template in order, relative to `project_root`.
///
/// # Errors
///
/// Returns E004, listing the tried paths, when no template names a file.
pub(crate) fn resolve(
    search_paths: &[String],
    project_root: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    let module_path = request.module.replace('.', "/");
    let mut tried_paths = Vec::new();

    for template in search_paths {
        let relative_path = template.replace('?', &module_path);
        let candidate = project_root.join(&relative_path);

        if candidate.is_file() {
            return Ok(ResolvedModule { path: normalize_path(&candidate), warnings: Vec::new() });
        }

        tried_paths.push(format!("`{relative_path}`"));
    }

    Err(Diagnostic::e004(request.from_file, request.span, request.module, &tried_paths))
}

#[cfg(test)]
mod tests {
    use luck_core::{Diagnostic, normalize_path};
    use luck_token::{LuaVersion, Span};
    use tempfile::TempDir;

    use crate::fixtures::project_with;
    use crate::{ResolveRequest, ResolvedModule, Resolver};

    fn lua_project() -> TempDir {
        project_with(&[
            ("src/main.lua", "local u = require('utils')"),
            ("src/utils.lua", "return {}"),
            ("src/foo/bar.lua", "return {}"),
            ("src/components/init.lua", "return {}"),
            ("lib/helper.lua", "return {}"),
        ])
    }

    /// Resolves `module`, required from `src/main.lua`, against `templates`
    /// rooted at `project`.
    ///
    /// # Errors
    ///
    /// Returns the resolver's diagnostic when `module` doesn't resolve.
    fn resolve_lua(
        project: &TempDir,
        templates: &[&str],
        module: &str,
    ) -> Result<ResolvedModule, Diagnostic> {
        let search_paths: Vec<String> =
            templates.iter().map(|template| (*template).to_owned()).collect();
        let from_file = normalize_path(&project.path().join("src/main.lua"));
        Resolver::new(LuaVersion::Lua54, &search_paths, project.path()).resolve(&ResolveRequest {
            module,
            from_file: &from_file,
            span: Span::new(0, 10),
        })
    }

    #[test]
    fn resolves_module_from_first_template() {
        let project = lua_project();
        let resolved = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "utils")
            .expect("resolve utils");
        assert_eq!(resolved.path, normalize_path(&project.path().join("src/utils.lua")));
    }

    #[test]
    fn resolves_dotted_module_to_nested_path() {
        let project = lua_project();
        let resolved = resolve_lua(&project, &["src/?.lua"], "foo.bar").expect("resolve foo.bar");
        assert_eq!(resolved.path, normalize_path(&project.path().join("src/foo/bar.lua")));
    }

    #[test]
    fn resolves_directory_module_via_init_template() {
        let project = lua_project();
        let resolved = resolve_lua(&project, &["src/?.lua", "src/?/init.lua"], "components")
            .expect("resolve components");
        assert_eq!(resolved.path, normalize_path(&project.path().join("src/components/init.lua")));
    }

    #[test]
    fn falls_back_to_later_template() {
        let project = lua_project();
        let resolved =
            resolve_lua(&project, &["src/?.lua", "lib/?.lua"], "helper").expect("resolve helper");
        assert_eq!(resolved.path, normalize_path(&project.path().join("lib/helper.lua")));
    }

    #[test]
    fn reports_e004_with_searched_paths_when_missing() {
        let project = lua_project();
        let error = resolve_lua(&project, &["src/?.lua"], "nonexistent")
            .expect_err("a missing module must not resolve");
        assert_eq!(error.code, "E004");
        assert!(
            error.help.as_deref().expect("e004 always sets help").contains("src/nonexistent.lua")
        );
    }
}
