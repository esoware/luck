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

    /// Whether a label may reuse the name of a label visible in an enclosing
    /// block. 5.4 made labels unique among the visible labels of a function.
    #[must_use]
    pub const fn has_label_shadowing(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 => true,
            Self::Lua51 | Self::Lua54 | Self::Lua55 | Self::Luau => false,
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

    /// Whether `global` declarations are supported (Lua 5.5). `global` stays
    /// an ordinary name, as in stock builds (`LUA_COMPAT_GLOBAL`), and the
    /// parser recognizes a declaration by the token after it.
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
    pub const fn has_last_stmt_break(self) -> bool {
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
    pub const fn has_empty_stmt(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether undefined escape sequences are lexer errors. Lua 5.1 and Luau
    /// treat any escaped non-digit character as that literal character
    /// (`"\m"` is `"m"`); 5.2+ reject them.
    #[must_use]
    pub const fn has_strict_escapes(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
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

    /// Whether a `.` stays in the numeral scan past a radix prefix or an
    /// exponent, so `0xFF..2` and `1e5..2` are malformed rather than concats.
    /// The 5.1 and Luau scanners stop before the dot there.
    #[must_use]
    pub const fn has_hex_scan_dots(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether any letter or `_` touching a decimal numeral makes it malformed.
    /// 5.2 and 5.3 read only hex digits past a numeral, so `1then` splits there
    /// into a number and a keyword.
    #[must_use]
    pub const fn has_strict_numeral_end(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 => false,
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

    /// Whether type annotations, casts, and declarations are supported.
    #[must_use]
    pub const fn has_type_syntax(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `@attr` is supported on functions.
    #[must_use]
    pub const fn has_function_attributes(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `if` can begin an expression.
    #[must_use]
    pub const fn has_if_exprs(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `const name = value` declares a local, as a contextual keyword.
    #[must_use]
    pub const fn has_const_stmt(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether some binding is read-only, so writing to it is a compile error:
    /// `<const>` and `<close>` locals, or Luau's `const`.
    #[must_use]
    pub const fn has_read_only_bindings(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 => false,
        }
    }

    /// Whether a lone CR ends a line and LFCR is one line break. Luau's lexer
    /// counts only LF and reads a lone CR as whitespace. See
    /// [`crate::line_breaks`] for the shared definition.
    #[must_use]
    pub const fn has_lfcr_line_break(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Luau => false,
        }
    }

    /// Whether loading a file skips a leading UTF-8 BOM (5.2+). 5.1 and Luau
    /// read it as a malformed first token.
    #[must_use]
    pub const fn has_bom_skip(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether a first line starting with `#` is skipped without a `!` after
    /// it. Luau's loader skips only a `#!` line longer than those two bytes.
    #[must_use]
    pub const fn has_bare_hash_shebang(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Luau => false,
        }
    }

    /// Whether a raw NUL byte ends the source. Luau's lexer reads 0 as the
    /// end of input, and PUC Lua lexes a NUL like any other byte.
    #[must_use]
    pub const fn has_nul_source_end(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a `[[` inside a level-0 long bracket is an error. Lua 5.1's
    /// default `LUA_COMPAT_LSTR` rejects the nesting.
    #[must_use]
    pub const fn has_long_bracket_nesting_error(self) -> bool {
        match self {
            Self::Lua51 => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => false,
        }
    }

    /// Whether a `\u{XXX}` escape reaches past U+10FFFF, up to 2^31 - 1
    /// (5.4+). 5.3 and Luau stop at U+10FFFF.
    #[must_use]
    pub const fn has_extended_unicode_escape(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether a `\u{XXX}` escape is limited to 16 digits, however small its
    /// value (Luau's `fixupQuotedString`).
    #[must_use]
    pub const fn has_unicode_escape_digit_limit(self) -> bool {
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
    /// 5.2+ leaves `package.loaded[name]` unset when the loader raises, and
    /// Luau's `require` library caches only a returned value. 5.1 leaves its
    /// sentinel behind and raises "loop or previous error" forever after.
    #[must_use]
    pub const fn has_require_error_retry(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
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
