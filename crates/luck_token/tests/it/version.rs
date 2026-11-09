use LuaVersion::{Lua51, Lua52, Lua53, Lua54, Lua55, Luau};
use luck_token::{LuaVersion, StdlibEnvironment};

fn versions_with(predicate: fn(LuaVersion) -> bool) -> Vec<LuaVersion> {
    LuaVersion::ALL.into_iter().filter(|version| predicate(*version)).collect()
}

#[test]
fn stmt_predicates_match_their_versions() {
    let lua52_and_later = [Lua52, Lua53, Lua54, Lua55];
    assert_eq!(versions_with(LuaVersion::has_goto), lua52_and_later);
    assert_eq!(versions_with(LuaVersion::has_empty_stmt), lua52_and_later);
    assert_eq!(versions_with(LuaVersion::has_global), [Lua55]);
    assert_eq!(versions_with(LuaVersion::has_const_for_variables), [Lua55]);
    assert_eq!(versions_with(LuaVersion::has_last_stmt_break), [Lua51, Luau]);
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
    assert_eq!(versions_with(LuaVersion::has_hex_scan_dots), [Lua52, Lua53, Lua54, Lua55, Luau]);
    assert_eq!(versions_with(LuaVersion::has_strict_numeral_end), [Lua51, Lua54, Lua55, Luau]);
}

#[test]
fn binding_predicates_match_their_versions() {
    assert_eq!(versions_with(LuaVersion::has_attributes), [Lua54, Lua55]);
    assert_eq!(versions_with(LuaVersion::has_leading_attributes), [Lua55]);
    assert_eq!(versions_with(LuaVersion::has_named_varargs), [Lua55]);
    assert_eq!(versions_with(LuaVersion::has_env_upvalue), [Lua52, Lua53, Lua54, Lua55]);
    assert_eq!(versions_with(LuaVersion::has_read_only_bindings), [Lua54, Lua55, Luau]);
    assert_eq!(versions_with(LuaVersion::has_explicit_type_instantiation), [Luau]);
    assert_eq!(versions_with(LuaVersion::has_type_syntax), [Luau]);
    assert_eq!(versions_with(LuaVersion::has_function_attributes), [Luau]);
    assert_eq!(versions_with(LuaVersion::has_if_exprs), [Luau]);
    assert_eq!(versions_with(LuaVersion::has_const_stmt), [Luau]);
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
    assert_eq!(versions_with(LuaVersion::has_lfcr_line_break), [Lua51, Lua52, Lua53, Lua54, Lua55]);
}

#[test]
fn source_predicates_match_their_versions() {
    let puc_versions = [Lua51, Lua52, Lua53, Lua54, Lua55];
    assert_eq!(versions_with(LuaVersion::has_bom_skip), [Lua52, Lua53, Lua54, Lua55]);
    assert_eq!(versions_with(LuaVersion::has_bare_hash_shebang), puc_versions);
    assert_eq!(versions_with(LuaVersion::has_nul_source_end), [Luau]);
    assert_eq!(versions_with(LuaVersion::has_long_bracket_nesting_error), [Lua51]);
    assert_eq!(versions_with(LuaVersion::has_extended_unicode_escape), [Lua54, Lua55]);
    assert_eq!(versions_with(LuaVersion::has_unicode_escape_digit_limit), [Luau]);
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

#[test]
fn bug_luau_reads_undefined_escapes_as_the_character() {
    assert!(
        !Luau.has_strict_escapes(),
        "Luau accepts an undefined escape as that character (`print(\"\\q\")` prints `q`), so it is not strict"
    );
}

#[test]
fn bug_luau_hex_numerals_stop_before_a_dot() {
    assert!(
        !Luau.has_hex_scan_dots(),
        "Luau reads `0xFF..2` as a number, a concat, and a number (`print(0xFF..2)` prints `2552`), so a dot is not part of its hex scan"
    );
}

#[test]
fn bug_luau_require_runs_a_failed_module_again() {
    assert!(
        Luau.has_require_error_retry(),
        "a module that raised is not cached by Luau's `require`, so a later `require` runs it again"
    );
}
