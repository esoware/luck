/// Target Lua dialect. Its predicates gate syntax and version-specific runtime
/// behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LuaVersion {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
    Luau,
}

impl LuaVersion {
    /// Every dialect, in release order.
    pub const ALL: [Self; 6] =
        [Self::Lua51, Self::Lua52, Self::Lua53, Self::Lua54, Self::Lua55, Self::Luau];

    /// Whether `goto` and labels are supported.
    #[must_use]
    pub const fn has_goto(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether `//` floor division is supported.
    #[must_use]
    pub const fn has_floor_div(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 => false,
        }
    }

    /// Whether bitwise operators (`&`, `|`, `~`, `<<`, `>>`) are supported.
    #[must_use]
    pub const fn has_bitwise_ops(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    /// Whether local variable attributes (`<const>`, `<close>`) are supported.
    #[must_use]
    pub const fn has_attributes(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether `global` declarations are supported (Lua 5.5).
    #[must_use]
    pub const fn has_global(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether global access desugars to an index of the `_ENV` upvalue
    /// (Lua 5.2+). Elsewhere a binding named `_ENV` is an ordinary local with
    /// no effect on the names around it.
    #[must_use]
    pub const fn has_env_upvalue(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether numbers have distinct integer/float subtypes (Lua 5.3+).
    /// Observable via `math.type`, `tostring` (`1` vs `1.0`), and `//`.
    /// Luau keeps a single f64 number type, like 5.1 and 5.2.
    #[must_use]
    pub const fn has_integer_subtype(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    /// Whether an attribute may precede the first name in an attnamelist
    /// (`local <const> x`, `global <const> y`). Lua 5.5 syntax.
    #[must_use]
    pub const fn has_leading_attributes(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether `break` is a last statement, valid only at the end of a block.
    /// Lua 5.1's grammar has `laststat ::= return | break`; Luau extends it
    /// with `continue` but keeps the last-statement restriction. 5.2+
    /// allows `break` anywhere.
    #[must_use]
    pub const fn has_last_statement_break(self) -> bool {
        match self {
            Self::Lua51 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a call's `(` on a new line is an "ambiguous syntax"
    /// parse error (5.1 and Luau). 5.2+ parses it as a call.
    #[must_use]
    pub const fn has_ambiguous_call_newline_error(self) -> bool {
        match self {
            Self::Lua51 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `;` is a valid empty statement (Lua 5.2+, not Luau).
    #[must_use]
    pub const fn has_empty_statement(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether undefined escape sequences are lexer errors. Lua 5.1
    /// treats any escaped non-digit character as that literal character
    /// (`"\m"` is `"m"`); 5.2+ and Luau reject them.
    #[must_use]
    pub const fn has_strict_escapes(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `\x` hex escape is supported in strings.
    #[must_use]
    pub const fn has_hex_escape(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `\z` whitespace-skip escape is supported in strings.
    #[must_use]
    pub const fn has_whitespace_escape(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `\u{XXXX}` unicode escape is supported in strings.
    #[must_use]
    pub const fn has_unicode_escape(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 => false,
        }
    }

    /// Whether hex float literals (`0x1.Fp10`) are supported.
    #[must_use]
    pub const fn has_hex_floats(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether binary literals (`0b1010`) are supported.
    #[must_use]
    pub const fn has_binary_literals(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether underscore separators in numbers (`1_000`) are supported.
    #[must_use]
    pub const fn has_underscore_separators(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether Luau's distinct 64-bit integer literals (`123i`) are supported.
    #[must_use]
    pub const fn has_luau_integer_literals(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether explicit type-parameter instantiation (`f<<T>>`) is supported.
    #[must_use]
    pub const fn has_explicit_type_instantiation(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `export` applies to runtime values as well as types (Luau).
    #[must_use]
    pub const fn has_value_exports(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    #[must_use]
    pub const fn is_luau(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether for-loop control variables are read-only, so assigning one is a
    /// compile error (Lua 5.5).
    #[must_use]
    pub const fn has_const_for_variables(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether named varargs (`...name`) are supported.
    #[must_use]
    pub const fn has_named_varargs(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether `continue` ends a loop iteration (Luau, as a contextual
    /// keyword).
    #[must_use]
    pub const fn has_continue(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether compound assignment operators (`+=`, `-=`, etc.) are supported.
    #[must_use]
    pub const fn has_compound_assignment(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether interpolated strings (`` `text{expr}` ``) are supported.
    #[must_use]
    pub const fn has_interpolated_strings(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a required module chunk receives the resolved file path as its
    /// second argument (Lua 5.2+). 5.1 loaders pass only the module name, and
    /// Luau's `require` calls the chunk with no arguments.
    #[must_use]
    pub const fn has_require_filepath_arg(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether `require` returns the loader data (the resolved file
    /// path, or ":preload:") as a second result on first load (5.4+).
    /// Cache hits still return a single value.
    #[must_use]
    pub const fn has_require_loaderdata(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether a failed module load may be retried by a later `require`.
    /// 5.2+ leaves `package.loaded[name]` unset when the loader raises;
    /// 5.1 leaves its sentinel behind and raises "loop or previous
    /// error" forever after, and Luau caches the failure.
    #[must_use]
    pub const fn has_require_error_retry(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether float `%` is computed as fmod plus a sign fix (5.3+).
    /// 5.1, 5.2, and Luau compute `a - floor(a/b)*b`, which loses
    /// precision at large magnitudes and the sign of zero results.
    #[must_use]
    pub const fn has_fmod_float_modulo(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    /// Whether the float `%` sign fix compares operand signs directly
    /// (5.4+). 5.3 tests `fmod(a,b)*b < 0`, whose product can underflow
    /// to zero for subnormal operands and skip the fix.
    #[must_use]
    pub const fn has_float_modulo_sign_compare(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }
}

/// The standard-library environment a Luau program targets. Only meaningful
/// for Luau; vanilla Lua versions always use `Standalone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdlibEnvironment {
    #[default]
    Standalone,
    Roblox,
}

impl StdlibEnvironment {
    #[must_use]
    pub const fn is_roblox(self) -> bool {
        matches!(self, Self::Roblox)
    }
}

#[cfg(test)]
mod tests {
    use super::LuaVersion::*;
    use super::*;

    fn versions_with(predicate: fn(LuaVersion) -> bool) -> Vec<LuaVersion> {
        LuaVersion::ALL.into_iter().filter(|version| predicate(*version)).collect()
    }

    #[test]
    fn statement_predicates_match_their_versions() {
        let lua52_and_later = [Lua52, Lua53, Lua54, Lua55];
        assert_eq!(versions_with(LuaVersion::has_goto), lua52_and_later);
        assert_eq!(versions_with(LuaVersion::has_empty_statement), lua52_and_later);
        assert_eq!(versions_with(LuaVersion::has_global), [Lua55]);
        assert_eq!(versions_with(LuaVersion::has_const_for_variables), [Lua55]);
        assert_eq!(versions_with(LuaVersion::has_last_statement_break), [Lua51, Luau]);
        assert_eq!(versions_with(LuaVersion::has_ambiguous_call_newline_error), [Lua51, Luau]);
        assert_eq!(versions_with(LuaVersion::has_continue), [Luau]);
        assert_eq!(versions_with(LuaVersion::has_compound_assignment), [Luau]);
        assert_eq!(versions_with(LuaVersion::has_value_exports), [Luau]);
    }

    #[test]
    fn operator_and_number_predicates_match_their_versions() {
        assert_eq!(versions_with(LuaVersion::has_floor_div), [Lua53, Lua54, Lua55, Luau]);
        let lua53_and_later = [Lua53, Lua54, Lua55];
        assert_eq!(versions_with(LuaVersion::has_bitwise_ops), lua53_and_later);
        assert_eq!(versions_with(LuaVersion::has_integer_subtype), lua53_and_later);
        assert_eq!(versions_with(LuaVersion::has_fmod_float_modulo), lua53_and_later);
        assert_eq!(versions_with(LuaVersion::has_float_modulo_sign_compare), [Lua54, Lua55]);
        assert_eq!(versions_with(LuaVersion::has_hex_floats), [Lua52, Lua53, Lua54, Lua55]);
        assert_eq!(versions_with(LuaVersion::has_binary_literals), [Luau]);
        assert_eq!(versions_with(LuaVersion::has_underscore_separators), [Luau]);
        assert_eq!(versions_with(LuaVersion::has_luau_integer_literals), [Luau]);
    }

    #[test]
    fn binding_predicates_match_their_versions() {
        assert_eq!(versions_with(LuaVersion::has_attributes), [Lua54, Lua55]);
        assert_eq!(versions_with(LuaVersion::has_leading_attributes), [Lua55]);
        assert_eq!(versions_with(LuaVersion::has_named_varargs), [Lua55]);
        assert_eq!(versions_with(LuaVersion::has_env_upvalue), [Lua52, Lua53, Lua54, Lua55]);
        assert_eq!(versions_with(LuaVersion::has_explicit_type_instantiation), [Luau]);
        assert_eq!(versions_with(LuaVersion::is_luau), [Luau]);
    }

    #[test]
    fn string_predicates_match_their_versions() {
        let all_but_lua51 = [Lua52, Lua53, Lua54, Lua55, Luau];
        assert_eq!(versions_with(LuaVersion::has_strict_escapes), all_but_lua51);
        assert_eq!(versions_with(LuaVersion::has_hex_escape), all_but_lua51);
        assert_eq!(versions_with(LuaVersion::has_whitespace_escape), all_but_lua51);
        assert_eq!(versions_with(LuaVersion::has_unicode_escape), [Lua53, Lua54, Lua55, Luau]);
        assert_eq!(versions_with(LuaVersion::has_interpolated_strings), [Luau]);
    }

    #[test]
    fn require_predicates_match_their_versions() {
        let lua52_and_later = [Lua52, Lua53, Lua54, Lua55];
        assert_eq!(versions_with(LuaVersion::has_require_filepath_arg), lua52_and_later);
        assert_eq!(versions_with(LuaVersion::has_require_error_retry), lua52_and_later);
        assert_eq!(versions_with(LuaVersion::has_require_loaderdata), [Lua54, Lua55]);
    }

    #[test]
    fn stdlib_environment_defaults_to_standalone_and_flags_only_roblox() {
        assert_eq!(StdlibEnvironment::default(), StdlibEnvironment::Standalone);
        assert!(!StdlibEnvironment::Standalone.is_roblox());
        assert!(StdlibEnvironment::Roblox.is_roblox());
    }
}
