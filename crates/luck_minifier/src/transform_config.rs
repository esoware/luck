/// Turns individual minifier passes on or off. Every pass except
/// `rename_globals` is on by default.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize), serde(default, deny_unknown_fields))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TransformConfig {
    /// Luau: drop every type annotation, cast, instantiation, and `type`
    /// declaration before the other passes run. Types never run, so this
    /// changes no behavior, but a typed library's users lose its types.
    pub strip_types: bool,
    pub remove_dead_code: bool,
    pub simplify_statements: bool,
    pub fold_constants: bool,
    pub inline_locals: bool,
    pub merge_locals: bool,
    pub simplify_indexes: bool,
    pub simplify_parens: bool,
    pub rename_locals: bool,
    pub lift_locals: bool,
    /// Rename globals this file defines, such as `function helper() end` or
    /// `counter = 0`. Off by default: renamed globals land under different
    /// `_G` keys, which breaks other chunks, `_G["name"]` lookups, and loaded
    /// strings that use the original names. Only takes effect with
    /// `rename_locals` on.
    pub rename_globals: bool,
}

impl TransformConfig {
    /// Whether any pass the compressor runs is on.
    pub(crate) const fn compresses(&self) -> bool {
        self.remove_dead_code
            || self.simplify_statements
            || self.fold_constants
            || self.inline_locals
            || self.merge_locals
            || self.simplify_indexes
            || self.simplify_parens
            || self.lift_locals
    }
}

impl Default for TransformConfig {
    fn default() -> Self {
        Self {
            strip_types: true,
            remove_dead_code: true,
            simplify_statements: true,
            fold_constants: true,
            inline_locals: true,
            merge_locals: true,
            simplify_indexes: true,
            simplify_parens: true,
            rename_locals: true,
            lift_locals: true,
            rename_globals: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_passes_but_not_rename_globals() {
        let TransformConfig {
            strip_types,
            remove_dead_code,
            simplify_statements,
            fold_constants,
            inline_locals,
            merge_locals,
            simplify_indexes,
            simplify_parens,
            rename_locals,
            lift_locals,
            rename_globals,
        } = TransformConfig::default();
        assert!(
            strip_types
                && remove_dead_code
                && simplify_statements
                && fold_constants
                && inline_locals
                && merge_locals
                && simplify_indexes
                && simplify_parens
                && rename_locals
                && lift_locals
        );
        assert!(!rename_globals);
    }
}
