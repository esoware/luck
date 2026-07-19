//! Module identity for the dependency graph.

use luck_ast::shared::Block;
use std::ops::Range;

/// Opaque identifier for a module in the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

/// Source file metadata: path, content, and discovered dependencies.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: String,
    pub source: String,
    /// (local_name, require_string, resolved_path, call_span)
    /// call_span is the byte range of the `require(...)` expression in source
    pub dependencies: Vec<(String, String, String, Range<usize>)>,
    pub sanitized_name: String,
    /// Cached parsed AST block, populated during graph construction to avoid re-parsing in the emitter
    pub parsed_block: Option<Block>,
}

/// Converts a file path into a valid Lua identifier prefixed with `__luck_`.
pub fn sanitize_module_name(path: &str) -> String {
    let mut name = path.to_string();

    if let Some(stripped) = name.strip_suffix(".luau") {
        name = stripped.to_string();
    } else if let Some(stripped) = name.strip_suffix(".lua") {
        name = stripped.to_string();
    }

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!("__luck_{sanitized}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_module_name() {
        assert_eq!(
            sanitize_module_name("foo/bar/baz.lua"),
            "__luck_foo_bar_baz"
        );
        assert_eq!(sanitize_module_name("src/utils.lua"), "__luck_src_utils");
        assert_eq!(
            sanitize_module_name("lib/my-module.lua"),
            "__luck_lib_my_module"
        );
        assert_eq!(sanitize_module_name("init.luau"), "__luck_init");
        assert_eq!(
            sanitize_module_name("foo.bar.baz.lua"),
            "__luck_foo_bar_baz"
        );
    }

    #[test]
    fn test_sanitize_module_name_backslash_paths() {
        assert_eq!(sanitize_module_name("src\\utils.lua"), "__luck_src_utils");
        assert_eq!(
            sanitize_module_name("C:\\project\\lib\\mod.luau"),
            "__luck_C__project_lib_mod"
        );
    }
}
