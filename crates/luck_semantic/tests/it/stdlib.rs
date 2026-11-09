//! Tests for the loaded stdlib catalog: per-version entries, arities, and
//! deprecations, the Roblox shapes and generated sets, and call resolution
//! through shapes.

use core::ptr;

use luck_ast::{Expression, LastStatement, Statement};
use luck_semantic::{
    StdlibArgKind, StdlibConstant, StdlibEntry, StdlibFunction, StdlibLibrary,
    expand_replace_template, library_for,
};
use luck_token::{BinOp, LuaVersion, StdlibEnvironment};

use crate::common::{NUMBERED_VERSIONS, all_libraries, luau, roblox, standalone_library};

fn function_at<'library>(
    library: &'library StdlibLibrary,
    path: &[&str],
) -> &'library StdlibFunction {
    let Some(StdlibEntry::Function(function)) = library.lookup(path) else {
        panic!("{:?}/{:?}: {path:?} should be a function", library.version, library.environment);
    };
    function
}

#[test]
fn every_library_loads() {
    for (file, library) in all_libraries() {
        assert!(library.globals.contains_key("print"), "{file} stdlib should expose print");
        assert!(library.globals.contains_key("type"), "{file} stdlib should expose type");
    }
}

#[test]
fn numbered_lua_environment_is_ignored() {
    // Only Luau distinguishes environments; a Roblox request for a
    // numbered version returns that version's single library.
    let standalone = library_for(LuaVersion::Lua54, StdlibEnvironment::Standalone);
    let roblox_environment = library_for(LuaVersion::Lua54, StdlibEnvironment::Roblox);
    assert!(ptr::eq(standalone, roblox_environment));
}

#[test]
fn table_getn_deprecated_in_51_and_fixes_to_length_from_52() {
    assert!(
        function_at(standalone_library(LuaVersion::Lua51), &["table", "getn"]).deprecated.is_some(),
        "Lua 5.1 marks table.getn deprecated in luck since it's already replaced by '#'"
    );

    for newer in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        let function = function_at(standalone_library(newer), &["table", "getn"]);
        let deprecation =
            function.deprecated.as_ref().expect("table.getn must be deprecated in 5.2+");
        let template = deprecation
            .replace_template
            .as_ref()
            .expect("table.getn deprecation must carry a replace template");
        assert_eq!(template.as_str(), "#%1");
    }
}

#[test]
fn unpack_deprecated_from_52() {
    assert!(
        function_at(standalone_library(LuaVersion::Lua51), &["unpack"]).deprecated.is_none(),
        "unpack is current in 5.1"
    );
    for newer in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let function = function_at(standalone_library(newer), &["unpack"]);
        let deprecation = function.deprecated.as_ref().expect("unpack deprecated in 5.2+");
        let template =
            deprecation.replace_template.as_ref().expect("unpack has a replace template");
        assert_eq!(template.as_str(), "table.unpack(%1)");
    }
}

#[test]
fn bit32_present_in_52_and_53_absent_in_51_and_54() {
    assert!(
        standalone_library(LuaVersion::Lua51).lookup(&["bit32"]).is_none(),
        "bit32 is not in Lua 5.1"
    );

    for present in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Luau] {
        let entry =
            standalone_library(present).lookup(&["bit32"]).expect("bit32 expected in 5.2/5.3/luau");
        assert!(matches!(entry, StdlibEntry::Namespace(_)), "bit32 should be a namespace");
    }
    assert!(
        standalone_library(LuaVersion::Lua54).lookup(&["bit32"]).is_none(),
        "bit32 was removed in Lua 5.4"
    );
}

#[test]
fn utf8_only_53_plus() {
    assert!(standalone_library(LuaVersion::Lua51).lookup(&["utf8"]).is_none());
    assert!(standalone_library(LuaVersion::Lua52).lookup(&["utf8"]).is_none());
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        assert!(
            standalone_library(version).lookup(&["utf8"]).is_some(),
            "utf8 expected in {version:?}"
        );
    }
    assert!(roblox().lookup(&["utf8"]).is_some());
}

#[test]
fn loadstring_deprecated_in_52_fixes_to_load() {
    let function = function_at(standalone_library(LuaVersion::Lua52), &["loadstring"]);
    let deprecation = function.deprecated.as_ref().expect("loadstring is deprecated");
    let template =
        deprecation.replace_template.as_ref().expect("loadstring has a replace template");
    assert_eq!(template.as_str(), "load(%1)");
}

#[test]
fn must_use_markers_present() {
    let lua54 = standalone_library(LuaVersion::Lua54);
    for path in [
        &["pcall"][..],
        &["tostring"][..],
        &["tonumber"][..],
        &["type"][..],
        &["select"][..],
        &["table", "concat"][..],
        &["coroutine", "create"][..],
    ] {
        assert!(function_at(lua54, path).is_must_use, "{path:?} should be must_use");
    }
}

#[test]
fn luau_buffer_and_vector_present() {
    for library in [luau(), roblox()] {
        assert!(library.lookup(&["buffer"]).is_some());
        assert!(library.lookup(&["buffer", "create"]).is_some());
        assert!(library.lookup(&["vector"]).is_some());
        assert!(library.lookup(&["vector", "create"]).is_some());
    }
    assert!(roblox().lookup(&["task", "spawn"]).is_some());
}

#[test]
fn luau_integer_library_and_buffer_accessors_are_present() {
    let integer_members = [
        "create",
        "fromstring",
        "tonumber",
        "neg",
        "add",
        "sub",
        "mul",
        "div",
        "rem",
        "idiv",
        "mod",
        "udiv",
        "urem",
        "bnot",
        "band",
        "bor",
        "bxor",
        "btest",
        "min",
        "max",
        "clamp",
        "lt",
        "le",
        "gt",
        "ge",
        "ult",
        "ule",
        "ugt",
        "uge",
        "lshift",
        "rshift",
        "arshift",
        "lrotate",
        "rrotate",
        "extract",
        "replace",
        "countrz",
        "countlz",
        "bswap",
        "maxsigned",
        "minsigned",
    ];

    for library in [luau(), roblox()] {
        assert!(library.lookup(&["integer"]).is_some());
        for member in integer_members {
            assert!(
                library.lookup(&["integer", member]).is_some(),
                "integer.{member} should be modeled"
            );
        }
        assert!(library.lookup(&["buffer", "readinteger"]).is_some());
        assert!(library.lookup(&["buffer", "writeinteger"]).is_some());
    }

    assert!(standalone_library(LuaVersion::Lua54).lookup(&["integer"]).is_none());
}

#[test]
fn luau_math_has_predicates_and_new_constants() {
    for library in [luau(), roblox()] {
        for name in ["isnan", "isinf", "isfinite"] {
            assert!(
                matches!(library.lookup(&["math", name]), Some(StdlibEntry::Function(_))),
                "math.{name} should be modeled as a function"
            );
        }
        for name in ["nan", "e", "phi", "sqrt2", "tau"] {
            assert!(
                matches!(library.lookup(&["math", name]), Some(StdlibEntry::Constant(_))),
                "math.{name} should be modeled as a constant"
            );
        }
    }
}

#[test]
fn luau_string_rep_has_no_separator() {
    for library in [luau(), roblox()] {
        assert_eq!(
            function_at(library, &["string", "rep"]).max_args(),
            Some(2),
            "Luau rep has no separator arg"
        );
    }
}

#[test]
fn luau_buffer_bit_functions_present() {
    for library in [luau(), roblox()] {
        for name in ["readbits", "writebits"] {
            assert!(library.lookup(&["buffer", name]).is_some(), "buffer.{name} missing");
        }
    }
}

#[test]
fn vector_create_wide_mode_standalone_only() {
    let standalone_function = function_at(luau(), &["vector", "create"]);
    assert!(standalone_function.accepts_arg_count(3));
    assert!(standalone_function.accepts_arg_count(4), "standalone wide mode takes w");

    let roblox_function = function_at(roblox(), &["vector", "create"]);
    assert!(roblox_function.accepts_arg_count(3));
    assert!(!roblox_function.accepts_arg_count(4), "roblox vectors are 3-wide");
}

#[test]
fn luau_io_package_removed_os_sandboxed() {
    for library in [luau(), roblox()] {
        assert!(library.lookup(&["io"]).is_none());
        assert!(library.lookup(&["package"]).is_none());
        // os survives as the sandboxed clock/date/difftime/time subset.
        assert!(library.lookup(&["os", "time"]).is_some());
        assert!(library.lookup(&["os", "clock"]).is_some());
        assert!(library.lookup(&["os", "getenv"]).is_none());
        assert!(library.lookup(&["os", "exit"]).is_none());
    }
}

#[test]
fn luau_table_clone_present_in_luau_only() {
    assert!(luau().lookup(&["table", "clone"]).is_some());
    assert!(roblox().lookup(&["table", "clone"]).is_some());

    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        assert!(
            standalone_library(version).lookup(&["table", "clone"]).is_none(),
            "table.clone should not exist in {version:?}"
        );
    }
}

#[test]
fn string_constant_arg_set_for_io_open_mode() {
    let second = function_at(standalone_library(LuaVersion::Lua54), &["io", "open"])
        .primary_signature()
        .params
        .get(1)
        .expect("io.open has at least 2 documented params");
    let StdlibArgKind::Constant(values) = &second.kind else {
        panic!("io.open's second arg should be a constant set");
    };
    assert!(values.iter().any(|constant| constant.value == "r"));
    assert!(values.iter().any(|constant| constant.value == "w"));
}

#[test]
fn pure_markers_present() {
    let lua54 = standalone_library(LuaVersion::Lua54);
    assert!(function_at(lua54, &["math", "abs"]).is_pure);
    assert!(!function_at(lua54, &["print"]).is_pure);
}

#[test]
fn roblox_entries_absent_from_standalone_luau() {
    let standalone = luau();
    let roblox_library = roblox();
    for segments in [
        &["warn"][..],
        &["task"][..],
        &["task", "spawn"][..],
        &["task", "synchronize"][..],
        &["task", "desynchronize"][..],
        &["utf8", "graphemes"][..],
        &["utf8", "nfcnormalize"][..],
        &["utf8", "nfdnormalize"][..],
        &["debug", "profilebegin"][..],
        &["debug", "profileend"][..],
        &["debug", "getmemorycategory"][..],
        &["debug", "setmemorycategory"][..],
        &["debug", "resetmemorycategory"][..],
        &["debug", "dumpcodesize"][..],
        &["game"][..],
        &["workspace"][..],
        &["script"][..],
        &["Vector3"][..],
        &["CFrame"][..],
        &["Instance"][..],
        &["Enum"][..],
        &["tick"][..],
        &["wait"][..],
        &["UserSettings"][..],
        &["PluginManager"][..],
        &["stats"][..],
        &["printidentity"][..],
        &["DebuggerManager"][..],
        &["ypcall"][..],
    ] {
        assert!(
            roblox_library.lookup(segments).is_some(),
            "{segments:?} must exist in the Roblox library"
        );
        assert!(
            standalone.lookup(segments).is_none(),
            "{segments:?} must be absent from standalone Luau"
        );
    }
}

#[test]
fn loadstring_present_in_both_luau_environments() {
    assert!(
        luau().lookup(&["loadstring"]).is_some(),
        "loadstring exists in the standalone Luau CLI"
    );
    assert!(
        roblox().lookup(&["loadstring"]).is_some(),
        "loadstring is a real Roblox global (gated by LoadStringEnabled)"
    );
}

#[test]
fn bit_namespace_absent_everywhere() {
    // No catalog has LuaJIT's 'bit' library, and Roblox never shipped it;
    // only bit32.
    for (file, library) in all_libraries() {
        assert!(library.lookup(&["bit"]).is_none(), "phantom bit namespace in {file}");
    }
}

#[test]
fn roblox_only_deprecations_respect_the_file_split() {
    for name in ["collectgarbage", "getfenv", "setfenv"] {
        let standalone_entry =
            luau().lookup(&[name]).unwrap_or_else(|| panic!("{name} in standalone"));
        assert!(
            standalone_entry.deprecation().is_none(),
            "{name} must not be deprecated in standalone Luau"
        );
        let roblox_entry = roblox().lookup(&[name]).unwrap_or_else(|| panic!("{name} on roblox"));
        assert!(roblox_entry.deprecation().is_some(), "{name} must be deprecated on Roblox");
    }
}

#[test]
fn legacy_roblox_globals_are_deprecated() {
    for name in ["tick", "version", "elapsedTime", "stats", "DebuggerManager"] {
        let entry = roblox().lookup(&[name]).unwrap_or_else(|| panic!("{name} present"));
        assert!(entry.deprecation().is_some(), "{name} must carry a deprecation marker");
    }
    let deprecation =
        function_at(roblox(), &["ypcall"]).deprecated.as_ref().expect("ypcall deprecated");
    assert_eq!(deprecation.replace_template.as_deref(), Some("pcall(%1)"), "ypcall fixes to pcall");
}

#[test]
fn core_luau_entries_in_both_environments() {
    // These come from the standard Luau VM, so both environments carry
    // their own copy.
    let checks: &[&[&str]] = &[
        &["typeof"],
        &["gcinfo"],
        &["getfenv"],
        &["setfenv"],
        &["newproxy"],
        &["math", "clamp"],
        &["math", "lerp"],
        &["math", "map"],
        &["math", "round"],
        &["math", "sign"],
        &["math", "noise"],
        &["string", "split"],
        &["table", "clone"],
        &["table", "freeze"],
        &["table", "isfrozen"],
        &["table", "find"],
        &["table", "clear"],
        &["table", "create"],
        &["bit32", "byteswap"],
        &["bit32", "countlz"],
        &["bit32", "countrz"],
        &["vector"],
        &["buffer"],
    ];
    for segments in checks {
        for library in [luau(), roblox()] {
            assert!(
                library.lookup(segments).is_some(),
                "{segments:?} missing from {:?}/{:?}",
                library.version,
                library.environment
            );
        }
    }
}

#[test]
fn lua51_xpcall_strictly_two_args() {
    let function = function_at(standalone_library(LuaVersion::Lua51), &["xpcall"]);
    assert_eq!(function.min_args(), 2);
    assert_eq!(
        function.max_args(),
        Some(2),
        "Lua 5.1 xpcall does not accept additional args (variadic form added in 5.2)"
    );
}

#[test]
fn lua52_package_searchers_replaces_loaders() {
    let lua52 = standalone_library(LuaVersion::Lua52);
    assert!(
        lua52.lookup(&["package", "searchers"]).is_some(),
        "package.searchers should exist in 5.2 (renamed from loaders)"
    );
    assert!(lua52.lookup(&["package", "loaders"]).is_none(), "package.loaders was removed in 5.2");
}

#[test]
fn lua53_randomseed_single_arg() {
    assert_eq!(
        function_at(standalone_library(LuaVersion::Lua53), &["math", "randomseed"]).max_args(),
        Some(1),
        "5.3 randomseed takes at most one arg; two-seed form is 5.4+"
    );
}

#[test]
fn lua54_debug_uservalue_optional_index() {
    let lua54 = standalone_library(LuaVersion::Lua54);
    assert_eq!(
        function_at(lua54, &["debug", "getuservalue"]).max_args(),
        Some(2),
        "5.4 added optional n index"
    );
    assert_eq!(
        function_at(lua54, &["debug", "setuservalue"]).max_args(),
        Some(3),
        "5.4 added optional n index"
    );
}

#[test]
fn lua52_debug_uservalue_and_upvalue_functions_present() {
    let lua52 = standalone_library(LuaVersion::Lua52);
    for (name, min, max) in
        [("getuservalue", 1, 1), ("setuservalue", 2, 2), ("upvalueid", 2, 2), ("upvaluejoin", 4, 4)]
    {
        let function = function_at(lua52, &["debug", name]);
        assert_eq!(function.min_args(), min, "debug.{name} min");
        assert_eq!(function.max_args(), Some(max), "debug.{name} max");
    }
    assert!(
        standalone_library(LuaVersion::Lua51).lookup(&["debug", "upvalueid"]).is_none(),
        "debug.upvalueid does not exist in 5.1"
    );
}

#[test]
fn math_compat_functions_not_deprecated_before_53() {
    // No early warnings: these are standard in 5.2 and deprecated only from
    // 5.3.
    for name in ["atan2", "cosh", "sinh", "tanh", "frexp", "ldexp", "pow"] {
        for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
            assert!(
                function_at(standalone_library(version), &["math", name]).deprecated.is_none(),
                "math.{name} is standard in {version:?}; no early-warning deprecation"
            );
        }
    }
    // math.log10 is different: 5.2 deprecates it and has its replacement.
    assert!(
        function_at(standalone_library(LuaVersion::Lua52), &["math", "log10"]).deprecated.is_some()
    );
}

#[test]
fn lua52_math_mod_present_deprecated_like_neighbors() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        assert!(
            function_at(standalone_library(version), &["math", "mod"]).deprecated.is_some(),
            "math.mod deprecated in {version:?}"
        );
    }
}

#[test]
fn lua53_bit32_members_all_deprecated() {
    let lua53 = standalone_library(LuaVersion::Lua53);
    let StdlibEntry::Namespace(namespace) = lua53.lookup(&["bit32"]).unwrap() else {
        panic!("bit32 should be a namespace");
    };
    for (name, entry) in &namespace.members {
        let StdlibEntry::Function(function) = entry else {
            continue;
        };
        assert!(
            function.deprecated.is_some(),
            "bit32.{name} must carry the compat deprecation in 5.3"
        );
    }
}

#[test]
fn randomseed_required_before_54_optional_after() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        assert_eq!(
            function_at(standalone_library(version), &["math", "randomseed"]).min_args(),
            1,
            "seed is required in {version:?}"
        );
    }
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let function = function_at(standalone_library(version), &["math", "randomseed"]);
        assert_eq!(function.min_args(), 0, "seed is optional in {version:?}");
        assert_eq!(function.max_args(), Some(2));
    }
}

#[test]
fn lua51_io_lines_filename_only() {
    assert_eq!(
        function_at(standalone_library(LuaVersion::Lua51), &["io", "lines"]).max_args(),
        Some(1),
        "read formats as io.lines arguments arrived in 5.2"
    );
    assert_eq!(
        function_at(standalone_library(LuaVersion::Lua52), &["io", "lines"]).max_args(),
        None,
        "5.2+ io.lines accepts read formats"
    );
}

#[test]
fn io_popen_mode_is_r_or_w_only() {
    for version in NUMBERED_VERSIONS {
        let function = function_at(standalone_library(version), &["io", "popen"]);
        let mode = &function.primary_signature().params[1];
        let StdlibArgKind::Constant(values) = &mode.kind else {
            panic!("io.popen mode should be a constant set");
        };
        let mut names: Vec<&str> = values.iter().map(|constant| constant.value.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["r", "w"], "io.popen mode set in {version:?}");
    }
}

#[test]
fn lua54_collectgarbage_overloads_and_constant_deprecation() {
    let function = function_at(standalone_library(LuaVersion::Lua54), &["collectgarbage"]);
    assert!(
        function.accepts_arg_count(4),
        "collectgarbage('incremental', p, s, sz) is valid in 5.4"
    );
    assert!(!function.accepts_arg_count(5));
    let four_arg_signatures: Vec<_> = function.matching_signatures(4).collect();
    assert_eq!(four_arg_signatures.len(), 1);
    let StdlibArgKind::Constant(four_arg_values) = &four_arg_signatures[0].params[0].kind else {
        panic!("first arg should be a constant set");
    };
    assert_eq!(four_arg_values.len(), 1);
    assert_eq!(four_arg_values[0].value, "incremental");

    let plain = function.primary_signature();
    let StdlibArgKind::Constant(plain_values) = &plain.params[0].kind else {
        panic!("plain options should be a constant set");
    };
    let setpause = plain_values
        .iter()
        .find(|constant| constant.value == "setpause")
        .expect("setpause still callable in 5.4");
    assert!(setpause.deprecated.is_some(), "5.4 manual 8.2 deprecates the setpause option");
    let collect = plain_values.iter().find(|constant| constant.value == "collect").unwrap();
    assert!(collect.deprecated.is_none());
}

#[test]
fn lua55_collectgarbage_param_form_replaces_setpause() {
    let function = function_at(standalone_library(LuaVersion::Lua55), &["collectgarbage"]);
    assert!(function.accepts_arg_count(3), "collectgarbage('param', 'pause', n) is valid in 5.5");
    for signature in &function.signatures {
        let StdlibArgKind::Constant(values) = &signature.params[0].kind else {
            continue;
        };
        assert!(
            values
                .iter()
                .all(|constant| constant.value != "setpause" && constant.value != "setstepmul"),
            "setpause/setstepmul were removed in 5.5"
        );
    }
    let three_arg_signatures: Vec<_> = function.matching_signatures(3).collect();
    assert_eq!(three_arg_signatures.len(), 1);
    let StdlibArgKind::Constant(names) = &three_arg_signatures[0].params[1].kind else {
        panic!("param name should be a constant set");
    };
    assert!(names.iter().any(|constant| constant.value == "pause"));
    assert!(names.iter().any(|constant| constant.value == "minormul"));
}

#[test]
fn allowed_constants_need_every_matching_signature_constrained() {
    let function = function_at(standalone_library(LuaVersion::Lua55), &["collectgarbage"]);
    let names =
        function.allowed_constants(1, 3).expect("the 3-arg `param` form takes a constant name");
    assert_eq!(names.len(), 1);
    assert!(names[0].iter().any(|constant| constant.value == "pause"));
    assert!(function.allowed_constants(2, 3).is_none(), "the `param` value is a number");
    assert!(function.allowed_constants(0, 9).is_none(), "no signature takes 9");

    let tostring = function_at(standalone_library(LuaVersion::Lua54), &["tostring"]);
    assert!(tostring.allowed_constants(0, 1).is_none());
}

#[test]
fn utf8_lax_flag_only_in_54_plus() {
    for (version, expected_len_max) in
        [(LuaVersion::Lua53, Some(3)), (LuaVersion::Lua54, Some(4)), (LuaVersion::Lua55, Some(4))]
    {
        let library = standalone_library(version);
        assert_eq!(
            function_at(library, &["utf8", "len"]).max_args(),
            expected_len_max,
            "utf8.len in {version:?}"
        );
        let expected = if version == LuaVersion::Lua53 { 1 } else { 2 };
        assert_eq!(
            function_at(library, &["utf8", "codes"]).max_args(),
            Some(expected),
            "utf8.codes in {version:?}"
        );
    }
}

#[test]
fn arg_global_present_in_all_numbered_versions() {
    for version in NUMBERED_VERSIONS {
        assert!(
            standalone_library(version).lookup(&["arg"]).is_some(),
            "the standalone interpreter defines arg in {version:?}"
        );
    }
}

#[test]
fn prompt_globals_follow_manual_documentation() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert!(
            standalone_library(version).lookup(&["_PROMPT"]).is_some(),
            "_PROMPT documented in {version:?}"
        );
        assert!(standalone_library(version).lookup(&["_PROMPT2"]).is_some());
    }
    // The 5.2 manual doesn't document the prompt overrides.
    assert!(standalone_library(LuaVersion::Lua52).lookup(&["_PROMPT"]).is_none());
}

#[test]
fn lua55_table_create_nrec_is_a_number_hint() {
    let function = function_at(standalone_library(LuaVersion::Lua55), &["table", "create"]);
    let nrec = &function.primary_signature().params[1];
    assert!(
        matches!(nrec.kind, StdlibArgKind::Number),
        "5.5 table.create takes a hash-size hint, not a Luau-style fill value"
    );
    assert!(!nrec.is_required);
}

#[test]
fn lua55_table_create_present_and_frexp_undeprecated() {
    let lua55 = standalone_library(LuaVersion::Lua55);
    assert!(lua55.lookup(&["table", "create"]).is_some(), "table.create was added in 5.5");
    assert!(
        function_at(lua55, &["math", "frexp"]).deprecated.is_none(),
        "5.5 made math.frexp standard again"
    );
    assert!(
        function_at(lua55, &["math", "ldexp"]).deprecated.is_none(),
        "5.5 made math.ldexp standard again"
    );
}

#[test]
fn roblox_datatypes_present_with_representative_statics() {
    let roblox_library = roblox();
    let standalone = luau();
    for segments in [
        &["Axes", "new"][..],
        &["Faces", "new"][..],
        &["CatalogSearchParams", "new"][..],
        &["ColorSequenceKeypoint", "new"][..],
        &["NumberSequenceKeypoint", "new"][..],
        &["Content", "fromUri"][..],
        &["Content", "fromAssetId"][..],
        &["Content", "fromObject"][..],
        &["Content", "none"][..],
        &["DockWidgetPluginGuiInfo", "new"][..],
        &["FloatCurveKey", "new"][..],
        &["RotationCurveKey", "new"][..],
        &["PathWaypoint", "new"][..],
        &["Path2DControlPoint", "new"][..],
        &["Region3int16", "new"][..],
        &["SecurityCapabilities", "new"][..],
        &["SecurityCapabilities", "fromCurrent"][..],
        &["SharedTable", "new"][..],
        &["SharedTable", "clear"][..],
        &["SharedTable", "clone"][..],
        &["SharedTable", "cloneAndFreeze"][..],
        &["SharedTable", "increment"][..],
        &["SharedTable", "isFrozen"][..],
        &["SharedTable", "size"][..],
        &["SharedTable", "update"][..],
        &["CFrame", "lookAlong"][..],
        &["CFrame", "fromRotationBetweenVectors"][..],
        &["CFrame", "fromEulerAngles"][..],
        &["CFrame", "fromEulerAnglesYXZ"][..],
        &["CFrame", "fromOrientation"][..],
        &["CFrame", "fromAxisAngle"][..],
        &["CFrame", "fromMatrix"][..],
        &["Vector3", "xAxis"][..],
        &["Vector3", "yAxis"][..],
        &["Vector3", "zAxis"][..],
        &["Vector3", "FromNormalId"][..],
        &["Vector3", "FromAxis"][..],
        &["Vector2", "xAxis"][..],
        &["Vector2", "yAxis"][..],
        &["BrickColor", "palette"][..],
        &["BrickColor", "White"][..],
        &["BrickColor", "Blue"][..],
        &["Font", "fromEnum"][..],
        &["Font", "fromId"][..],
        &["Font", "fromName"][..],
        &["DateTime", "fromUnixTimestampMillis"][..],
        &["DateTime", "fromUniversalTime"][..],
        &["DateTime", "fromLocalTime"][..],
        &["DateTime", "fromIsoDate"][..],
    ] {
        assert!(
            roblox_library.lookup(segments).is_some(),
            "{segments:?} must exist in the Roblox library"
        );
        assert!(
            standalone.lookup(segments).is_none(),
            "{segments:?} must be absent from standalone Luau"
        );
    }
}

#[test]
fn datetime_has_no_new_constructor() {
    assert!(
        roblox().lookup(&["DateTime", "new"]).is_none(),
        "DateTime only has factory statics; .new is a phantom"
    );
}

#[test]
fn region3_requires_both_corners() {
    let function = function_at(roblox(), &["Region3", "new"]);
    assert_eq!(function.min_args(), 2);
    assert_eq!(function.max_args(), Some(2));
}

#[test]
fn cframe_new_overloads_accept_documented_arities_only() {
    let function = function_at(roblox(), &["CFrame", "new"]);
    for count in [0, 1, 2, 3, 7, 12] {
        assert!(function.accepts_arg_count(count), "CFrame.new must accept {count} args");
    }
    for count in [4, 5, 6, 8, 11, 13] {
        assert!(!function.accepts_arg_count(count), "CFrame.new must reject {count} args");
    }
}

#[test]
fn instance_new_parent_param_is_deprecated() {
    let parent = &function_at(roblox(), &["Instance", "new"]).primary_signature().params[1];
    assert!(
        parent.deprecated.is_some(),
        "Instance.new's parent arg carries a per-param deprecation"
    );
}

#[test]
fn physical_properties_overloads_match_docs() {
    let function = function_at(roblox(), &["PhysicalProperties", "new"]);
    for count in [1, 3, 5, 6] {
        assert!(
            function.accepts_arg_count(count),
            "PhysicalProperties.new must accept {count} args"
        );
    }
    for count in [0, 2, 4, 7] {
        assert!(
            !function.accepts_arg_count(count),
            "PhysicalProperties.new must reject {count} args"
        );
    }
}

fn shape_function<'library>(
    library: &'library StdlibLibrary,
    shape: &str,
    name: &str,
) -> &'library StdlibFunction {
    let Some(StdlibEntry::Function(function)) = library.shape_member(shape, name) else {
        panic!(
            "{:?}/{:?}: shape {shape} should have function member {name}",
            library.version, library.environment
        );
    };
    function
}

#[test]
fn file_shape_present_in_all_numbered_versions() {
    for version in NUMBERED_VERSIONS {
        let library = standalone_library(version);
        for member in ["read", "lines", "write", "seek", "setvbuf", "close", "flush"] {
            let function = shape_function(library, "file", member);
            assert!(function.is_method, "{version:?}: file:{member} is a method");
        }
        for opener in [
            &["io", "open"][..],
            &["io", "popen"][..],
            &["io", "tmpfile"][..],
            &["io", "input"][..],
            &["io", "output"][..],
        ] {
            assert_eq!(
                function_at(library, opener).returns_shape.as_deref(),
                Some("file"),
                "{version:?}: {opener:?} returns a file"
            );
        }
        for stream in ["stderr", "stdin", "stdout"] {
            let Some(StdlibEntry::Property(value)) = library.lookup(&["io", stream]) else {
                panic!("{version:?}: io.{stream} missing");
            };
            assert_eq!(value.shape.as_deref(), Some("file"));
        }
    }
}

#[test]
fn file_method_version_deltas() {
    // 5.1 file:lines takes no formats and file:write returns nothing;
    // both changed in 5.2.
    let lines_51 = shape_function(standalone_library(LuaVersion::Lua51), "file", "lines");
    assert_eq!(lines_51.max_args(), Some(0));
    let write_51 = shape_function(standalone_library(LuaVersion::Lua51), "file", "write");
    assert!(write_51.returns_shape.is_none());
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let lines = shape_function(standalone_library(version), "file", "lines");
        assert_eq!(lines.max_args(), None, "{version:?}: lines takes formats");
        let write = shape_function(standalone_library(version), "file", "write");
        assert_eq!(write.returns_shape.as_deref(), Some("file"), "{version:?}");
    }
}

#[test]
fn string_receiver_shape_derived_in_every_library() {
    for (file, library) in all_libraries() {
        let upper = shape_function(library, "string", "upper");
        assert!(upper.is_method);
        assert_eq!(upper.min_args(), 0, "subject param dropped");
        assert_eq!(upper.returns_shape.as_deref(), Some("string"));
        let find = shape_function(library, "string", "find");
        assert!(find.min_args() >= 1, "pattern still required");
        for excluded in ["char", "dump"] {
            assert!(
                library.shape_member("string", excluded).is_none(),
                "{file}: string.{excluded} is not subject-first"
            );
        }
    }
}

#[test]
fn derived_rep_arity_follows_source_entry() {
    // rep(s, n) in 5.1 and Luau; rep(s, n, sep) in 5.2+.
    assert_eq!(
        shape_function(standalone_library(LuaVersion::Lua51), "string", "rep").max_args(),
        Some(1)
    );
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            shape_function(standalone_library(version), "string", "rep").max_args(),
            Some(2),
            "{version:?}"
        );
    }
    assert_eq!(shape_function(luau(), "string", "rep").max_args(), Some(1));
}

#[test]
fn derived_gfind_method_keeps_deprecation() {
    let gfind = shape_function(standalone_library(LuaVersion::Lua51), "string", "gfind");
    assert!(gfind.deprecated.is_some());
}

#[test]
fn luau_files_have_no_file_shape() {
    assert!(!luau().shapes.contains_key("file"));
    assert!(!roblox().shapes.contains_key("file"));
}

fn constant_set<'library>(
    library: &'library StdlibLibrary,
    segments: &[&str],
    param_index: usize,
) -> Option<&'library [StdlibConstant]> {
    let params = &function_at(library, segments).primary_signature().params;
    let StdlibArgKind::Constant(values) = &params.get(param_index)?.kind else {
        return None;
    };
    Some(values)
}

fn find<'values>(values: &'values [StdlibConstant], name: &str) -> Option<&'values StdlibConstant> {
    values.iter().find(|constant| constant.value == name)
}

#[test]
fn game_is_a_shaped_datamodel() {
    let get_service = function_at(roblox(), &["game", "GetService"]);
    assert!(get_service.is_method);
    assert!(get_service.is_must_use);
    assert_eq!(get_service.returns_shape.as_deref(), Some("Instance"));
    assert!(roblox().lookup(&["game", "FindService"]).is_some());
    assert!(roblox().lookup(&["game", "BindToClose"]).is_some());
    assert!(roblox().lookup(&["game", "IsLoaded"]).is_some());
}

#[test]
fn datamodel_and_workspace_extend_instance() {
    // Members inherited from the Instance base shape.
    assert!(roblox().lookup(&["game", "FindFirstChild"]).is_some());
    assert!(roblox().lookup(&["workspace", "WaitForChild"]).is_some());
    assert!(roblox().lookup(&["workspace", "Raycast"]).is_some());
    assert!(roblox().lookup(&["workspace", "CurrentCamera"]).is_some());
    assert!(roblox().lookup(&["script", "GetFullName"]).is_some());
    // game.Workspace chains into the Workspace shape.
    assert!(roblox().lookup(&["game", "Workspace", "Raycast"]).is_some());
    // None of this exists standalone.
    assert!(luau().lookup(&["game"]).is_none());
}

#[test]
fn service_set_sentinels() {
    let services = constant_set(roblox(), &["game", "GetService"], 0)
        .expect("game:GetService takes a constant set");
    assert!(services.len() > 300, "{}", services.len());
    assert!(find(services, "Players").expect("Players").is_common);
    assert!(find(services, "RunService").is_some());
    assert!(find(services, "ProximityPromptService").is_some());
    assert!(
        find(services, "PointsService").expect("PointsService").deprecated.is_some(),
        "dead services carry a deprecation marker"
    );
    // game:GetService can't reach the settings providers' services.
    assert!(find(services, "RenderSettings").is_none());
    assert!(find(services, "UserGameSettings").is_none());
}

#[test]
fn class_set_sentinels() {
    let creatable =
        constant_set(roblox(), &["Instance", "new"], 0).expect("Instance.new takes a constant set");
    assert!(find(creatable, "Folder").is_some());
    assert!(find(creatable, "Part").is_some());
    assert!(
        find(creatable, "Workspace").is_none(),
        "NotCreatable classes are not Instance.new-able"
    );
    assert!(find(creatable, "BasePart").is_none());
    assert!(
        find(creatable, "Hint").expect("Hint").deprecated.is_some(),
        "deprecated creatable classes carry a marker"
    );
    let is_a =
        constant_set(roblox(), &["script", "IsA"], 0).expect("script:IsA takes a constant set");
    assert!(find(is_a, "BasePart").is_some(), "IsA accepts abstract classes");
    assert!(find(is_a, "Instance").is_some());
}

#[test]
fn brickcolor_name_set() {
    let function = function_at(roblox(), &["BrickColor", "new"]);
    let StdlibArgKind::Constant(names) = &function.signatures[0].params[0].kind else {
        panic!("BrickColor.new arg 0 should be the name set");
    };
    assert_eq!(names.len(), 204);
    assert!(find(names, "Bright red").is_some());
    assert!(find(names, "Really black").is_some());
}

#[test]
fn settings_provider_shapes() {
    let function = shape_function(roblox(), "GlobalSettings", "GetService");
    let StdlibArgKind::Constant(values) = &function.primary_signature().params[0].kind else {
        panic!("expected constant set");
    };
    assert!(find(values, "Studio").is_some());
    assert!(find(values, "RenderSettings").is_some());
    let user_function = shape_function(roblox(), "UserSettings", "GetService");
    let StdlibArgKind::Constant(user_values) = &user_function.primary_signature().params[0].kind
    else {
        panic!("expected constant set");
    };
    assert!(find(user_values, "UserGameSettings").is_some());
}

/// Resolves the callee of the last statement's call, either a call
/// statement or the last initializer of a local assignment. Yields the
/// display path.
fn resolve_last_call(source: &str) -> Option<String> {
    let parsed = luck_parser::parse(source, LuaVersion::Luau);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let analysis = luck_semantic::analyze_with_environment(
        &parsed.block,
        LuaVersion::Luau,
        StdlibEnvironment::Roblox,
    );
    let last_stmt = parsed.block.stmts.last()?;
    let call = if let Statement::FunctionCall(call) = last_stmt {
        call
    } else if let Statement::LocalAssignment(local) = last_stmt
        && let Some(Expression::FunctionCall(call)) = local.exprs.as_ref()?.iter().last()
    {
        call
    } else {
        return None;
    };
    Some(analysis.resolve_callee(call)?.name())
}

#[test]
fn get_service_call_resolves() {
    let name = resolve_last_call("game:GetService('Players')").expect("game:GetService resolves");
    assert_eq!(name, "game:GetService");
}

#[test]
fn get_service_chain_resolves() {
    let name = resolve_last_call(
        "local players = game:GetService('Players')\nlocal x = players:FindFirstChild('x')",
    )
    .expect("chained FindFirstChild resolves");
    assert_eq!(name, "players:FindFirstChild");
}

#[test]
fn script_parent_chain_resolves() {
    let name = resolve_last_call(
        "local container = script.Parent\nlocal x = container:WaitForChild('Config')",
    )
    .expect("script.Parent chain resolves");
    assert_eq!(name, "container:WaitForChild");
}

#[test]
fn user_settings_call_chain_resolves() {
    let name = resolve_last_call("UserSettings():GetService('UserGameSettings')")
        .expect("UserSettings():GetService resolves");
    assert_eq!(name, "UserSettings:GetService");
}

#[test]
fn workspace_raycast_resolves() {
    let name = resolve_last_call("workspace:Raycast(origin, direction)")
        .expect("workspace:Raycast resolves");
    assert_eq!(name, "workspace:Raycast");
}

#[test]
fn enum_get_enum_items_resolves() {
    let name = resolve_last_call("local items = Enum.Material:GetEnumItems()")
        .expect("Enum.Material:GetEnumItems resolves");
    assert_eq!(name, "Enum:GetEnumItems");
}

#[test]
fn enum_from_name_resolves() {
    let name = resolve_last_call("local item = Enum.Material:FromName('Grass')")
        .expect("Enum.Material:FromName resolves");
    assert_eq!(name, "Enum:FromName");
}

#[test]
fn enum_global_get_enums_resolves() {
    let name = resolve_last_call("local all = Enum:GetEnums()").expect("Enum:GetEnums resolves");
    assert_eq!(name, "Enum:GetEnums");
}

#[test]
fn enum_item_name_is_string_shaped() {
    // EnumItem.Name carries the string shape, so string methods
    // chain off it.
    let name = resolve_last_call("local upper = Enum.Material.Grass.Name:upper()")
        .expect("Name:upper() resolves through the string shape");
    assert_eq!(name, "string:upper");
}

#[test]
fn enum_tree_is_populated() {
    let StdlibEntry::Namespace(enum_global) =
        roblox().lookup(&["Enum"]).expect("Enum global on Roblox")
    else {
        panic!("Enum should be a namespace");
    };
    assert!(
        enum_global.members.len() > 500,
        "expected the full generated tree, got {} types",
        enum_global.members.len()
    );
    assert_eq!(enum_global.shape.as_deref(), Some("Enums"));
}

#[test]
fn representative_items_resolve() {
    for path in [
        &["Enum", "Material", "Grass"][..],
        &["Enum", "KeyCode", "A"][..],
        &["Enum", "EasingStyle", "Linear"][..],
        &["Enum", "Font", "SourceSans"][..],
    ] {
        let entry = roblox().lookup(path).unwrap_or_else(|| panic!("missing {path:?}"));
        let StdlibEntry::Constant(value) = entry else {
            panic!("{path:?} should be a constant");
        };
        assert!(value.is_read_only);
        assert_eq!(value.shape.as_deref(), Some("EnumItem"));
    }
}

#[test]
fn enum_types_are_enum_shaped() {
    let StdlibEntry::Namespace(material) =
        roblox().lookup(&["Enum", "Material"]).expect("Enum.Material")
    else {
        panic!("Enum.Material should be a namespace");
    };
    assert_eq!(material.shape.as_deref(), Some("Enum"));
}

#[test]
fn deprecated_item_carries_marker() {
    let entry =
        roblox().lookup(&["Enum", "AlignType", "Parallel"]).expect("Enum.AlignType.Parallel");
    assert!(entry.deprecation().is_some(), "AlignType.Parallel is tagged Deprecated in the dump");
}

#[test]
fn item_properties_reachable_through_shape() {
    for member in ["Name", "Value", "EnumType"] {
        assert!(
            roblox().lookup(&["Enum", "Material", "Grass", member]).is_some(),
            "EnumItem member {member} should resolve through the shape"
        );
    }
}

#[test]
fn standalone_luau_has_no_enum() {
    assert!(luau().lookup(&["Enum"]).is_none());
}

#[test]
fn string_gmatch_accepts_init_from_54() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let library = standalone_library(version);
        assert!(
            function_at(library, &["string", "gmatch"]).accepts_arg_count(3),
            "{version:?}: string.gmatch (s, pattern [, init]) takes an init position since 5.4"
        );
        assert!(
            shape_function(library, "string", "gmatch").accepts_arg_count(2),
            "{version:?}: s:gmatch(pattern, init) takes an init position since 5.4"
        );
    }
}

#[test]
fn coroutine_isyieldable_accepts_coroutine_from_54() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert!(
            function_at(standalone_library(version), &["coroutine", "isyieldable"])
                .accepts_arg_count(1),
            "{version:?}: coroutine.isyieldable ([co]) takes a coroutine since 5.4"
        );
    }
}

#[test]
fn lua55_coroutine_close_coroutine_is_optional() {
    assert!(
        function_at(standalone_library(LuaVersion::Lua55), &["coroutine", "close"])
            .accepts_arg_count(0),
        "the 5.5 manual documents coroutine.close ([co]), which closes the running coroutine"
    );
}

#[test]
fn io_open_mode_rejects_b_before_plus_from_52() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let modes = constant_set(standalone_library(version), &["io", "open"], 1)
            .expect("io.open's mode is a constant set");
        for mode in ["rb+", "wb+", "ab+"] {
            assert!(
                find(modes, mode).is_none(),
                "{version:?}: io.open raises `invalid mode` for {mode}, since l_checkmode \
                 requires [rwa]%+?b*"
            );
        }
    }
}

#[test]
fn lua52_module_is_deprecated_not_removed() {
    let module = standalone_library(LuaVersion::Lua52).lookup(&["module"]);
    assert!(
        module.is_some_and(|entry| entry.deprecation().is_some()),
        "the 5.2 manual, section 8.2, deprecates module and the stock build still defines it, \
         so the catalog must keep it marked deprecated"
    );
}

#[test]
fn lua52_collectgarbage_accepts_setmajorinc() {
    let options = constant_set(standalone_library(LuaVersion::Lua52), &["collectgarbage"], 0)
        .expect("collectgarbage's option is a constant set");
    assert!(
        find(options, "setmajorinc").is_some(),
        "Lua 5.2's collectgarbage accepts the setmajorinc option"
    );
}

#[test]
fn luau_collectgarbage_accepts_collect() {
    let options = constant_set(luau(), &["collectgarbage"], 0)
        .expect("collectgarbage's option is a constant set");
    assert!(
        find(options, "collect").is_some(),
        "the standalone Luau CLI's collectgarbage accepts collect, and it is the default"
    );
}

#[test]
fn luau_collectgarbage_is_not_must_use() {
    assert!(
        !function_at(luau(), &["collectgarbage"]).is_must_use,
        "collectgarbage() runs a collection and returns nothing in the standalone Luau CLI"
    );
}

#[test]
fn luau_math_randomseed_requires_a_seed() {
    for library in [luau(), roblox()] {
        assert_eq!(
            function_at(library, &["math", "randomseed"]).min_args(),
            1,
            "math.randomseed calls luaL_checkinteger on its seed, so it can't be omitted"
        );
    }
}

#[test]
fn luau_vector_create_accepts_two_components() {
    assert!(
        function_at(luau(), &["vector", "create"]).accepts_arg_count(2),
        "the 2-component constructor RFC is implemented: vector.create(x, y) zeroes z"
    );
}

#[test]
fn luau_vector_lerp_is_cataloged() {
    for library in [luau(), roblox()] {
        assert!(
            matches!(library.lookup(&["vector", "lerp"]), Some(StdlibEntry::Function(_))),
            "{:?}: vector.lerp is an implemented RFC and in the Roblox docs",
            library.environment
        );
    }
}

#[test]
fn luau_coroutine_finally_is_cataloged() {
    for library in [luau(), roblox()] {
        assert!(
            matches!(library.lookup(&["coroutine", "finally"]), Some(StdlibEntry::Function(_))),
            "{:?}: coroutine.finally is in the Luau VM and the Roblox docs",
            library.environment
        );
    }
}

#[test]
fn tostring_is_not_pure() {
    for (file, library) in all_libraries() {
        assert!(
            !function_at(library, &["tostring"]).is_pure,
            "{file}: tostring runs the __tostring metamethod, so it isn't pure"
        );
    }
}

#[test]
fn metamethod_iterators_are_not_pure_from_52() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert!(
            !function_at(standalone_library(version), &["pairs"]).is_pure,
            "{version:?}: pairs runs the __pairs metamethod, so it isn't pure"
        );
    }
    assert!(
        !function_at(standalone_library(LuaVersion::Lua52), &["ipairs"]).is_pure,
        "Lua 5.2's ipairs runs the __ipairs metamethod, so it isn't pure"
    );
}

#[test]
fn table_maxn_has_no_length_operator_fix() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let function = function_at(standalone_library(version), &["table", "maxn"]);
        let deprecation = function.deprecated.as_ref().expect("table.maxn is deprecated");
        assert!(
            deprecation.replace_template.is_none(),
            "{version:?}: table.maxn({{[5] = 1}}) is 5 but #{{[5] = 1}} is 0, so `#` can't \
             replace it automatically"
        );
    }
}

fn lua53_bit32_replacement(name: &str) -> Option<&'static str> {
    let function = function_at(standalone_library(LuaVersion::Lua53), &["bit32", name]);
    function.deprecated.as_ref()?.replace_template.as_deref()
}

#[test]
fn bit32_bnot_fix_keeps_the_32_bit_result() {
    assert_ne!(
        lua53_bit32_replacement("bnot"),
        Some("(~%1)"),
        "bit32.bnot(0) is 4294967295 but ~0 is -1 on 64-bit integers"
    );
}

#[test]
fn bit32_arshift_fix_is_not_a_logical_shift() {
    assert_ne!(
        lua53_bit32_replacement("arshift"),
        Some("(%1 >> %2)"),
        "bit32.arshift(0x80000000, 1) is 3221225472 but 0x80000000 >> 1 is 1073741824, since \
         `>>` is a logical shift"
    );
}

#[test]
fn bit32_lshift_fix_keeps_the_32_bit_result() {
    assert_ne!(
        lua53_bit32_replacement("lshift"),
        Some("(%1 << %2)"),
        "bit32.lshift(0xFFFFFFFF, 4) is 4294967280 but 0xFFFFFFFF << 4 is 68719476720"
    );
}

#[test]
fn bit32_rshift_fix_keeps_the_32_bit_result() {
    assert_ne!(
        lua53_bit32_replacement("rshift"),
        Some("(%1 >> %2)"),
        "bit32.rshift(-1, 1) is 2147483647 but -1 >> 1 is 9223372036854775807"
    );
}

#[test]
fn luau_unpack_is_not_deprecated() {
    for library in [luau(), roblox()] {
        assert!(
            function_at(library, &["unpack"]).deprecated.is_none(),
            "{:?}: luau.org documents unpack as equivalent to table.unpack, and neither it nor \
             the Roblox globals page marks it deprecated",
            library.environment
        );
    }
}

fn fix_template<'library>(
    library: &'library StdlibLibrary,
    path: &[&str],
) -> Option<&'library str> {
    function_at(library, path).deprecated.as_ref()?.replace_template.as_deref()
}

/// The expression of `return <text>`, without its outer parentheses.
fn returned_expr(text: &str, version: LuaVersion) -> Expression {
    let parsed = luck_parser::parse(format!("return {text}"), version);
    assert!(parsed.errors.is_empty(), "{text}: {:?}", parsed.errors);
    let Some(LastStatement::Return(return_stmt)) = parsed.block.last_stmt.as_deref() else {
        panic!("{text} must parse as a return");
    };
    let mut expr = return_stmt.exprs.first().expect("one returned value").clone();
    while let Expression::Parenthesized(mut parenthesized) = expr {
        expr = parenthesized.expr.take();
    }
    expr
}

#[test]
fn math_pow_fix_groups_a_compound_base() {
    let version = LuaVersion::Lua53;
    let Some(template) = fix_template(standalone_library(version), &["math", "pow"]) else {
        return;
    };
    let replacement = expand_replace_template(template, &["a + 1", "2"]);
    assert!(
        matches!(
            returned_expr(&replacement, version),
            Expression::BinaryOp(binary_op) if binary_op.op == BinOp::Pow
        ),
        "with a = 2, math.pow(a + 1, 2) is 9.0, so the fix must raise the whole base, but \
         `{replacement}` is 3.0"
    );
}

#[test]
fn table_getn_fix_groups_a_compound_operand() {
    let version = LuaVersion::Lua51;
    let Some(template) = fix_template(standalone_library(version), &["table", "getn"]) else {
        return;
    };
    let replacement = expand_replace_template(template, &["a or b"]);
    assert!(
        matches!(returned_expr(&replacement, version), Expression::UnaryOp(_)),
        "table.getn(a or b) takes the length of `a or b`, but `{replacement}` takes the length \
         of a alone"
    );
}

#[test]
fn unpack_fix_keeps_the_range_arguments() {
    let Some(template) = fix_template(standalone_library(LuaVersion::Lua52), &["unpack"]) else {
        return;
    };
    let replacement = expand_replace_template(template, &["list", "first_index", "last_index"]);
    assert!(
        replacement.contains("first_index") && replacement.contains("last_index"),
        "unpack({{1, 2, 3}}, 2, 3) yields 2 3 and table.unpack({{1, 2, 3}}) yields 1 2 3, so \
         the fix can't drop the range: `{replacement}`"
    );
}

#[test]
fn loadstring_fix_keeps_the_chunk_name() {
    let Some(template) = fix_template(standalone_library(LuaVersion::Lua52), &["loadstring"])
    else {
        return;
    };
    let replacement = expand_replace_template(template, &["code", "chunk_name"]);
    assert!(
        replacement.contains("chunk_name"),
        "loadstring(code, name) labels the chunk's errors with name, so the fix can't drop it: \
         `{replacement}`"
    );
}

#[test]
fn ypcall_fix_keeps_the_forwarded_arguments() {
    let Some(template) = fix_template(roblox(), &["ypcall"]) else {
        return;
    };
    let replacement = expand_replace_template(template, &["callback", "forwarded"]);
    assert!(
        replacement.contains("forwarded"),
        "ypcall(f, x) calls f with x, so the fix can't drop x: `{replacement}`"
    );
}
