//! Module identity for the dependency graph.

use luck_ast::shared::Block;
use std::ops::Range;

/// Opaque identifier for a module in the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

/// A resolved `require()` edge out of a module.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// The literal require string as written in source (`require("foo")` -> `foo`).
    pub require_string: String,
    /// The normalized path the require resolved to; the graph's canonical module key.
    pub resolved_path: String,
    /// Byte range of the `require(...)` call expression in source, for
    /// bundler-side diagnostics (cycle reporting) that render against source.
    pub call_span: Range<usize>,
}

/// Source file metadata: path, content, and discovered dependencies.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: String,
    pub source: String,
    pub dependencies: Vec<Dependency>,
    pub sanitized_name: String,
    /// Parsed AST block, cached during graph construction to avoid re-parsing in the emitter.
    pub parsed_block: Option<Block>,
}

/// Converts a file path into a valid Lua identifier prefixed with `__luck_`.
pub fn sanitize_module_name(path: &str) -> String {
    let stem = path
        .strip_suffix(".luau")
        .or_else(|| path.strip_suffix(".lua"))
        .unwrap_or(path);

    let mut sanitized = String::with_capacity("__luck_".len() + stem.len());
    sanitized.push_str("__luck_");
    for byte in stem.chars() {
        if byte.is_ascii_alphanumeric() || byte == '_' {
            sanitized.push(byte);
        } else {
            sanitized.push('_');
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_path_separators_and_strips_extension() {
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
    fn sanitizes_backslash_paths() {
        assert_eq!(sanitize_module_name("src\\utils.lua"), "__luck_src_utils");
        assert_eq!(
            sanitize_module_name("C:\\project\\lib\\mod.luau"),
            "__luck_C__project_lib_mod"
        );
    }
}
