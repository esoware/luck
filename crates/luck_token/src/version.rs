/// Locals active at once in one function, the same in every dialect:
/// `MAXVARS` in PUC Lua and `kMaxLocalCount` in Luau.
pub const MAX_ACTIVE_LOCALS: usize = 200;

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

    pub const fn is_luau(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether a lone CR ends a line and LFCR is one line break. Luau's lexer
    /// counts only LF and reads a lone CR as whitespace. See
    /// [`crate::line_breaks`] for the shared definition.
    pub const fn has_lfcr_line_break(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Luau => false,
        }
    }

    /// 5.1 and Luau read it as a malformed first token.
    pub const fn has_bom_skip(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether a first line starting with `#` is skipped without a `!` after
    /// it. Luau's loader skips only a `#!` line longer than those two bytes.
    pub const fn has_bare_hash_shebang(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Luau => false,
        }
    }

    /// Whether a line comment opening with `--!`, such as `--!strict`, is a
    /// hot comment, which Luau reads before the first token.
    pub const fn has_hot_comments(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Luau's lexer reads 0 as the end of input, and PUC Lua lexes a NUL like any other byte.
    pub const fn has_nul_source_end(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_hex_floats(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    pub const fn has_binary_literals(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_underscore_separators(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether Luau's distinct 64-bit integer literals (`123i`) are supported.
    pub const fn has_luau_integer_literals(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a hex or binary numeral past 64 bits reads as 2^64, as Luau's
    /// `strtoull` saturates. PUC Lua rounds it to the nearest double (5.1,
    /// 5.2) or wraps it as an integer (5.3+).
    pub const fn has_saturating_hex_numerals(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a `.` stays in the numeral scan past a radix prefix or an
    /// exponent, so `0xFF..2` and `1e5..2` are malformed rather than concats.
    /// The 5.1 and Luau scanners stop before the dot there.
    pub const fn has_hex_scan_dots(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether any letter or `_` touching a decimal numeral makes it malformed.
    /// 5.2 and 5.3 read only hex digits past a numeral, so `1then` splits there
    /// into a number and a keyword.
    pub const fn has_strict_numeral_end(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 => false,
        }
    }

    /// Lua 5.1 and Luau treat any escaped non-digit character as that literal character
    /// (`"\m"` is `"m"`); 5.2+ reject them.
    pub const fn has_strict_escapes(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    pub const fn has_hex_escape(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    pub const fn has_whitespace_escape(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    pub const fn has_unicode_escape(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 => false,
        }
    }

    /// Whether a `\u{XXX}` escape reaches past U+10FFFF, up to 2^31 - 1
    /// (5.4+). 5.3 and Luau stop at U+10FFFF.
    pub const fn has_extended_unicode_escape(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether a `\u{XXX}` escape is limited to 16 digits, however small its
    /// value (Luau's `fixupQuotedString`).
    pub const fn has_unicode_escape_digit_limit(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Lua 5.1's default `LUA_COMPAT_LSTR` rejects the nesting.
    pub const fn has_long_bracket_nesting_error(self) -> bool {
        match self {
            Self::Lua51 => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => false,
        }
    }

    pub const fn has_interpolated_strings(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_floor_div(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 => false,
        }
    }

    pub const fn has_bitwise_ops(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    pub const fn has_if_exprs(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_goto(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// 5.4 made labels unique among the visible labels of a function.
    pub const fn has_label_shadowing(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 => true,
            Self::Lua51 | Self::Lua54 | Self::Lua55 | Self::Luau => false,
        }
    }

    pub const fn has_empty_stmt(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Lua 5.1's grammar has `laststat ::= return | break`; Luau extends it
    /// with `continue` but keeps the last-statement restriction. 5.2+
    /// allows `break` anywhere.
    pub const fn has_last_stmt_break(self) -> bool {
        match self {
            Self::Lua51 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether a call's `(` on a new line is an "ambiguous syntax"
    /// parse error (5.1 and Luau). 5.2+ parses it as a call.
    pub const fn has_ambiguous_call_newline_error(self) -> bool {
        match self {
            Self::Lua51 | Self::Luau => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `continue` ends a loop iteration (Luau, as a contextual
    /// keyword).
    pub const fn has_continue(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_compound_assignment(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// `global` stays an ordinary name, as in stock builds (`LUA_COMPAT_GLOBAL`), and the
    /// parser recognizes a declaration by the token after it.
    pub const fn has_global(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether `const name = value` declares a local, as a contextual keyword.
    pub const fn has_const_stmt(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_value_exports(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_attributes(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether an attribute may precede the first name in an attnamelist
    /// (`local <const> x`, `global <const> y`).
    pub const fn has_leading_attributes(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    pub const fn has_const_for_variables(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether some binding is read-only, so writing to it is a compile error:
    /// `<const>` and `<close>` locals, or Luau's `const`.
    pub const fn has_read_only_bindings(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 => false,
        }
    }

    /// Whether global access desugars to an index of the `_ENV` upvalue
    /// (Lua 5.2+). Elsewhere a binding named `_ENV` is an ordinary local with
    /// no effect on the names around it.
    pub const fn has_env_upvalue(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    pub const fn has_named_varargs(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether a vararg function whose `...` has no name holds its extra
    /// arguments in a hidden local, `(vararg table)` in 5.5's `parlist`.
    pub const fn has_hidden_vararg_table(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// Whether a vararg function binds an implicit `arg` table even when
    /// nothing reads it, as stock 5.1 builds do under `LUA_COMPAT_VARARG`.
    pub const fn has_implicit_vararg_table(self) -> bool {
        match self {
            Self::Lua51 => true,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => false,
        }
    }

    pub const fn has_type_syntax(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_explicit_type_instantiation(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    pub const fn has_function_attributes(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Observable via `math.type`, `tostring` (`1` vs `1.0`), and `//`.
    /// Luau keeps a single f64 number type, like 5.1 and 5.2.
    pub const fn has_integer_subtype(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    /// Whether float `%` is computed as fmod plus a sign fix (5.3+).
    /// 5.1, 5.2, and Luau compute `a - floor(a/b)*b`, which loses
    /// precision at large magnitudes and the sign of zero results.
    pub const fn has_fmod_float_modulo(self) -> bool {
        match self {
            Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Luau => false,
        }
    }

    /// Whether the float `%` sign fix compares operand signs directly
    /// (5.4+). 5.3 tests `fmod(a,b)*b < 0`, whose product can underflow
    /// to zero for subnormal operands and skip the fix.
    pub const fn has_float_modulo_sign_compare(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether the compiler folds a `<const>` local whose value is a
    /// constant into its uses, so it takes no register and no function
    /// captures it (`RDKCTC`, 5.4+).
    pub const fn has_compile_time_constants(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether the compiler counts a function's locals and upvalues against
    /// its limits as it parses, as PUC Lua's one-pass compiler does. Luau's
    /// counts what its optimizer leaves, which depends on the level it
    /// compiles at.
    pub const fn has_parse_time_limits(self) -> bool {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Luau => false,
        }
    }

    /// Whether [`MAX_ACTIVE_LOCALS`] counts only the locals in registers,
    /// which leaves compile-time constants out, as 5.5's `adjustlocalvars`
    /// does. Earlier versions count every local a function declares.
    pub const fn has_register_local_limit(self) -> bool {
        match self {
            Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Luau => false,
        }
    }

    /// The hidden locals a numeric `for` declares before its variable, which
    /// count toward [`MAX_ACTIVE_LOCALS`]. Luau's compiler keeps a loop's
    /// state in registers that are no locals.
    pub const fn numeric_for_state_locals(self) -> usize {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 => 3,
            Self::Lua55 => 2,
            Self::Luau => 0,
        }
    }

    /// The hidden locals a generic `for` declares before its variables, which
    /// count toward [`MAX_ACTIVE_LOCALS`]: the iterator, its state, and its
    /// control value, and in 5.4 the value it closes.
    pub const fn generic_for_state_locals(self) -> usize {
        match self {
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua55 => 3,
            Self::Lua54 => 4,
            Self::Luau => 0,
        }
    }

    /// The upvalues one function can capture: `LUAI_MAXUPVALUES` in 5.1,
    /// `MAXUPVAL` in 5.2+, and `kMaxUpvalueCount` in Luau.
    pub const fn max_upvalues(self) -> usize {
        match self {
            Self::Lua51 => 60,
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => 255,
            Self::Luau => 200,
        }
    }

    /// Whether a required module chunk receives the resolved file path as its
    /// second argument (Lua 5.2+). 5.1 loaders pass only the module name, and
    /// Luau's `require` calls the chunk with no arguments.
    pub const fn has_require_filepath_arg(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether `require` returns the loader data (the resolved file
    /// path, or ":preload:") as a second result on first load (5.4+).
    /// Cache hits still return a single value.
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
    pub const fn has_require_error_retry(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `string.format` takes `%a` and `%A` (5.2+, not Luau).
    pub const fn has_hex_float_format(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }

    /// Whether `string.format` takes `%p` (5.4+).
    pub const fn has_pointer_format(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether `string.format` takes `%*`, which formats any value (Luau).
    pub const fn has_star_format(self) -> bool {
        match self {
            Self::Luau => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => false,
        }
    }

    /// Whether `string.format` checks a specifier's flags, width, and
    /// precision against its conversion, as 5.4's `checkformat` does. Earlier
    /// versions and Luau only limit how many there are.
    pub const fn has_checked_format_modifiers(self) -> bool {
        match self {
            Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Luau => false,
        }
    }

    /// Whether a `gsub` replacement string raises an error on a `%` before
    /// anything but a digit or `%`, a trailing one included. Lua 5.1 copies
    /// the following character.
    pub const fn has_strict_gsub_replacement(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `os.date` checks its conversion specifiers. Lua 5.1 passes
    /// the whole format to `strftime`.
    pub const fn has_checked_date_format(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => true,
            Self::Lua51 => false,
        }
    }

    /// Whether `os.date` reads specifiers through a `checkoption` table,
    /// with C99 and two-character forms and an error on a trailing `%`
    /// (5.2+). Luau checks one C89 list and keeps a trailing `%`.
    pub const fn has_date_format_options(self) -> bool {
        match self {
            Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 => true,
            Self::Lua51 | Self::Luau => false,
        }
    }
}
