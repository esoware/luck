//! Integration tests for the rich stdlib model. Verifies that:
//!   - each per-environment TOML file loads without panicking
//!   - key entries are present per Lua version
//!   - deprecation/`must_use` metadata round-trips through the loader
//!   - version-gated entries appear and disappear as expected
//!   - the standalone and Roblox Luau libraries diverge where they must

use compact_str::CompactString;
use luck_semantic::stdlib_model::{StdlibEntry, StdlibLibrary, library_for};
use luck_token::{LuaVersion, StdlibEnvironment};

fn path(segments: &[&str]) -> Vec<CompactString> {
    segments.iter().copied().map(CompactString::from).collect()
}

/// The single library of a numbered Lua version (or standalone Luau).
fn lib(version: LuaVersion) -> &'static StdlibLibrary {
    library_for(version, StdlibEnvironment::Standalone)
}

fn roblox() -> &'static StdlibLibrary {
    library_for(LuaVersion::Luau, StdlibEnvironment::Roblox)
}

#[test]
fn every_library_loads() {
    let mut libraries: Vec<&StdlibLibrary> = [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ]
    .into_iter()
    .map(lib)
    .collect();
    libraries.push(roblox());
    for library in libraries {
        assert!(
            library.globals.contains_key("print"),
            "{:?}/{:?} stdlib should expose print",
            library.version,
            library.environment
        );
        assert!(
            library.globals.contains_key("type"),
            "{:?}/{:?} stdlib should expose type",
            library.version,
            library.environment
        );
    }
}

#[test]
fn numbered_lua_environment_is_ignored() {
    // Only Luau distinguishes environments; a Roblox request for a
    // numbered version returns that version's single library.
    let standalone = library_for(LuaVersion::Lua54, StdlibEnvironment::Standalone);
    let roblox_env = library_for(LuaVersion::Lua54, StdlibEnvironment::Roblox);
    assert!(std::ptr::eq(standalone, roblox_env));
}

#[test]
fn table_getn_only_deprecated_in_52_plus() {
    let lua51 = lib(LuaVersion::Lua51);
    let getn_51 = lua51
        .lookup(&path(&["table", "getn"]))
        .expect("table.getn present in 5.1");
    match getn_51 {
        StdlibEntry::Function(func) => {
            assert!(
                func.deprecated.is_some(),
                "Lua 5.1 marks table.getn deprecated in luck since it's already replaced by '#'"
            );
            // Luck's policy: even in 5.1, prefer `#` so we suggest it.
        }
        _ => panic!("table.getn should be a function"),
    }

    for newer in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        let entry = lib(newer)
            .lookup(&path(&["table", "getn"]))
            .expect("table.getn still visible in newer versions");
        match entry {
            StdlibEntry::Function(func) => {
                let deprecation = func
                    .deprecated
                    .as_ref()
                    .expect("table.getn must be deprecated in 5.2+");
                let template = deprecation
                    .replace_template
                    .as_ref()
                    .expect("table.getn deprecation must carry a replace template");
                assert_eq!(template.as_str(), "#%1");
            }
            _ => panic!("table.getn should be a function"),
        }
    }
}

#[test]
fn unpack_deprecated_from_52() {
    let lua51 = lib(LuaVersion::Lua51);
    let unpack_51 = lua51.lookup(&path(&["unpack"])).expect("unpack in 5.1");
    if let StdlibEntry::Function(func) = unpack_51 {
        assert!(func.deprecated.is_none(), "unpack is current in 5.1");
    }
    for newer in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let entry = lib(newer)
            .lookup(&path(&["unpack"]))
            .expect("unpack kept as compat in 5.2/5.3");
        let StdlibEntry::Function(func) = entry else {
            panic!("unpack should be a function");
        };
        let deprecation = func.deprecated.as_ref().expect("unpack deprecated in 5.2+");
        let template = deprecation
            .replace_template
            .as_ref()
            .expect("unpack has a replace template");
        assert_eq!(template.as_str(), "table.unpack(%1)");
    }
}

#[test]
fn bit32_present_in_52_and_53_absent_in_51_and_54() {
    assert!(
        lib(LuaVersion::Lua51).lookup(&path(&["bit32"])).is_none(),
        "bit32 is not in Lua 5.1"
    );

    for present in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Luau] {
        let entry = lib(present)
            .lookup(&path(&["bit32"]))
            .expect("bit32 expected in 5.2/5.3/luau");
        assert!(
            matches!(entry, StdlibEntry::Namespace(_)),
            "bit32 should be a namespace"
        );
    }
    assert!(
        lib(LuaVersion::Lua54).lookup(&path(&["bit32"])).is_none(),
        "bit32 was removed in Lua 5.4"
    );
}

#[test]
fn utf8_only_53_plus() {
    assert!(lib(LuaVersion::Lua51).lookup(&path(&["utf8"])).is_none());
    assert!(lib(LuaVersion::Lua52).lookup(&path(&["utf8"])).is_none());
    for v in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        assert!(
            lib(v).lookup(&path(&["utf8"])).is_some(),
            "utf8 expected in {v:?}"
        );
    }
    assert!(roblox().lookup(&path(&["utf8"])).is_some());
}

#[test]
fn loadstring_replace_template_round_trip() {
    let entry = lib(LuaVersion::Lua52)
        .lookup(&path(&["loadstring"]))
        .expect("loadstring in 5.2");
    if let StdlibEntry::Function(func) = entry {
        let deprecation = func.deprecated.as_ref().expect("loadstring is deprecated");
        let template = deprecation
            .replace_template
            .as_ref()
            .expect("loadstring has a replace template");
        assert_eq!(template.as_str(), "load(%1)");
    } else {
        panic!("loadstring must be a function");
    }
}

#[test]
fn must_use_markers_present() {
    let lua54 = lib(LuaVersion::Lua54);
    for name in ["pcall", "tostring", "tonumber", "type", "select"] {
        let entry = lua54.lookup(&path(&[name])).expect(name);
        if let StdlibEntry::Function(func) = entry {
            assert!(func.must_use, "{name} should be must_use");
        }
    }
    let table_concat = lua54.lookup(&path(&["table", "concat"])).unwrap();
    if let StdlibEntry::Function(func) = table_concat {
        assert!(func.must_use);
    }
    let coro_create = lua54.lookup(&path(&["coroutine", "create"])).unwrap();
    if let StdlibEntry::Function(func) = coro_create {
        assert!(func.must_use);
    }
}

#[test]
fn luau_buffer_and_vector_present() {
    for library in [lib(LuaVersion::Luau), roblox()] {
        assert!(library.lookup(&path(&["buffer"])).is_some());
        assert!(library.lookup(&path(&["buffer", "create"])).is_some());
        assert!(library.lookup(&path(&["vector"])).is_some());
        assert!(library.lookup(&path(&["vector", "create"])).is_some());
    }
    assert!(roblox().lookup(&path(&["task", "spawn"])).is_some());
}

#[test]
fn luau_math_has_no_isnan_family() {
    for library in [lib(LuaVersion::Luau), roblox()] {
        for name in ["isnan", "isinf", "isfinite"] {
            assert!(
                library.lookup(&path(&["math", name])).is_none(),
                "math.{name} does not exist in Luau"
            );
        }
    }
}

#[test]
fn luau_string_rep_has_no_separator() {
    for library in [lib(LuaVersion::Luau), roblox()] {
        let entry = library
            .lookup(&path(&["string", "rep"]))
            .expect("string.rep");
        let StdlibEntry::Function(func) = entry else {
            panic!("string.rep should be a function");
        };
        assert_eq!(func.max_args(), Some(2), "Luau rep has no separator arg");
    }
}

#[test]
fn luau_buffer_bit_functions_present() {
    for library in [lib(LuaVersion::Luau), roblox()] {
        for name in ["readbits", "writebits"] {
            assert!(
                library.lookup(&path(&["buffer", name])).is_some(),
                "buffer.{name} missing"
            );
        }
    }
}

#[test]
fn vector_create_wide_mode_standalone_only() {
    let standalone = lib(LuaVersion::Luau)
        .lookup(&path(&["vector", "create"]))
        .expect("vector.create standalone");
    let StdlibEntry::Function(func) = standalone else {
        panic!("vector.create should be a function");
    };
    assert!(func.accepts_arg_count(3));
    assert!(func.accepts_arg_count(4), "standalone wide mode takes w");

    let entry = roblox()
        .lookup(&path(&["vector", "create"]))
        .expect("vector.create roblox");
    let StdlibEntry::Function(func) = entry else {
        panic!("vector.create should be a function");
    };
    assert!(func.accepts_arg_count(3));
    assert!(!func.accepts_arg_count(4), "roblox vectors are 3-wide");
}

#[test]
fn luau_io_package_removed_os_sandboxed() {
    for library in [lib(LuaVersion::Luau), roblox()] {
        assert!(library.lookup(&path(&["io"])).is_none());
        assert!(library.lookup(&path(&["package"])).is_none());
        // os survives as the sandboxed clock/date/difftime/time subset.
        assert!(library.lookup(&path(&["os", "time"])).is_some());
        assert!(library.lookup(&path(&["os", "clock"])).is_some());
        assert!(library.lookup(&path(&["os", "getenv"])).is_none());
        assert!(library.lookup(&path(&["os", "exit"])).is_none());
    }
}

#[test]
fn luau_table_clone_present_in_luau_only() {
    assert!(
        lib(LuaVersion::Luau)
            .lookup(&path(&["table", "clone"]))
            .is_some()
    );
    assert!(roblox().lookup(&path(&["table", "clone"])).is_some());

    // Standard Lua doesn't have table.clone.
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
    ] {
        assert!(
            lib(v).lookup(&path(&["table", "clone"])).is_none(),
            "table.clone should not exist in {v:?}"
        );
    }
}

#[test]
fn string_constant_arg_set_for_io_open_mode() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    let io_open = lib(LuaVersion::Lua54)
        .lookup(&path(&["io", "open"]))
        .unwrap();
    let StdlibEntry::Function(func) = io_open else {
        panic!("io.open should be a function");
    };
    let second = func
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
fn pure_markers_preserved() {
    let lua54 = lib(LuaVersion::Lua54);
    let math_abs = lua54.lookup(&path(&["math", "abs"])).unwrap();
    if let StdlibEntry::Function(func) = math_abs {
        assert!(func.is_pure);
    }
    let print = lua54.lookup(&path(&["print"])).unwrap();
    if let StdlibEntry::Function(func) = print {
        assert!(!func.is_pure);
    }
}

#[test]
fn roblox_entries_absent_from_standalone_luau() {
    let standalone = lib(LuaVersion::Luau);
    let roblox_lib = roblox();
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
            roblox_lib.lookup_str(segments).is_some(),
            "{segments:?} must exist in the Roblox library"
        );
        assert!(
            standalone.lookup_str(segments).is_none(),
            "{segments:?} must be absent from standalone Luau"
        );
    }
}

#[test]
fn loadstring_present_in_both_luau_environments() {
    assert!(
        lib(LuaVersion::Luau)
            .lookup(&path(&["loadstring"]))
            .is_some(),
        "loadstring exists in the standalone Luau CLI"
    );
    assert!(
        roblox().lookup(&path(&["loadstring"])).is_some(),
        "loadstring is a real Roblox global (gated by LoadStringEnabled)"
    );
}

#[test]
fn bit_namespace_absent_everywhere() {
    // Roblox never shipped a LuaJIT-style 'bit' library; only bit32.
    assert!(lib(LuaVersion::Luau).lookup(&path(&["bit"])).is_none());
    assert!(roblox().lookup(&path(&["bit"])).is_none());
}

#[test]
fn roblox_only_deprecations_respect_the_file_split() {
    for name in ["collectgarbage", "getfenv", "setfenv"] {
        let standalone_entry = lib(LuaVersion::Luau)
            .lookup(&path(&[name]))
            .unwrap_or_else(|| panic!("{name} in standalone"));
        assert!(
            standalone_entry.deprecation().is_none(),
            "{name} must not be deprecated in standalone Luau"
        );
        let roblox_entry = roblox()
            .lookup(&path(&[name]))
            .unwrap_or_else(|| panic!("{name} on roblox"));
        assert!(
            roblox_entry.deprecation().is_some(),
            "{name} must be deprecated on Roblox"
        );
    }
}

#[test]
fn legacy_roblox_globals_are_deprecated() {
    for name in ["tick", "version", "elapsedTime", "stats", "DebuggerManager"] {
        let entry = roblox()
            .lookup(&path(&[name]))
            .unwrap_or_else(|| panic!("{name} present"));
        assert!(
            entry.deprecation().is_some(),
            "{name} must carry a deprecation marker"
        );
    }
    let ypcall = roblox().lookup(&path(&["ypcall"])).expect("ypcall present");
    let StdlibEntry::Function(func) = ypcall else {
        panic!("ypcall must be a function");
    };
    let deprecation = func.deprecated.as_ref().expect("ypcall deprecated");
    assert_eq!(
        deprecation.replace_template.as_deref(),
        Some("pcall(%1)"),
        "ypcall fixes to pcall"
    );
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
        for library in [lib(LuaVersion::Luau), roblox()] {
            assert!(
                library.lookup_str(segments).is_some(),
                "{segments:?} missing from {:?}/{:?}",
                library.version,
                library.environment
            );
        }
    }
}

#[test]
fn lua51_xpcall_strictly_two_args() {
    let xpcall = lib(LuaVersion::Lua51)
        .lookup(&path(&["xpcall"]))
        .expect("xpcall in 5.1");
    let StdlibEntry::Function(func) = xpcall else {
        panic!("xpcall must be a function");
    };
    assert_eq!(func.min_args(), 2);
    assert_eq!(
        func.max_args(),
        Some(2),
        "Lua 5.1 xpcall does not accept additional args (variadic form added in 5.2)"
    );
}

#[test]
fn lua52_package_searchers_replaces_loaders() {
    let lua52 = lib(LuaVersion::Lua52);
    assert!(
        lua52.lookup(&path(&["package", "searchers"])).is_some(),
        "package.searchers should exist in 5.2 (renamed from loaders)"
    );
    assert!(
        lua52.lookup(&path(&["package", "loaders"])).is_none(),
        "package.loaders was removed in 5.2"
    );
}

#[test]
fn lua53_randomseed_single_arg() {
    let rs = lib(LuaVersion::Lua53)
        .lookup(&path(&["math", "randomseed"]))
        .expect("math.randomseed in 5.3");
    if let StdlibEntry::Function(func) = rs {
        assert_eq!(
            func.max_args(),
            Some(1),
            "5.3 randomseed takes at most one arg; two-seed form is 5.4+"
        );
    }
}

#[test]
fn lua54_debug_uservalue_optional_index() {
    let lua54 = lib(LuaVersion::Lua54);
    let get = lua54.lookup(&path(&["debug", "getuservalue"])).unwrap();
    if let StdlibEntry::Function(func) = get {
        assert_eq!(func.max_args(), Some(2), "5.4 added optional n index");
    }
    let set = lua54.lookup(&path(&["debug", "setuservalue"])).unwrap();
    if let StdlibEntry::Function(func) = set {
        assert_eq!(func.max_args(), Some(3), "5.4 added optional n index");
    }
}

#[test]
fn lua52_debug_uservalue_and_upvalue_functions_present() {
    let lua52 = lib(LuaVersion::Lua52);
    for (name, min, max) in [
        ("getuservalue", 1, 1),
        ("setuservalue", 2, 2),
        ("upvalueid", 2, 2),
        ("upvaluejoin", 4, 4),
    ] {
        let entry = lua52
            .lookup(&path(&["debug", name]))
            .unwrap_or_else(|| panic!("debug.{name} was added in 5.2"));
        let StdlibEntry::Function(func) = entry else {
            panic!("debug.{name} should be a function");
        };
        assert_eq!(func.min_args(), min, "debug.{name} min");
        assert_eq!(func.max_args(), Some(max), "debug.{name} max");
    }
    assert!(
        lib(LuaVersion::Lua51)
            .lookup(&path(&["debug", "upvalueid"]))
            .is_none(),
        "debug.upvalueid does not exist in 5.1"
    );
}

#[test]
fn lua52_math_compat_functions_not_deprecated() {
    // User policy: no early warnings. These are fully standard in 5.2;
    // their removal happened in 5.3.
    for name in ["atan2", "cosh", "sinh", "tanh", "frexp", "ldexp", "pow"] {
        for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
            let entry = lib(version)
                .lookup(&path(&["math", name]))
                .unwrap_or_else(|| panic!("math.{name} in {version:?}"));
            let StdlibEntry::Function(func) = entry else {
                panic!("math.{name} should be a function");
            };
            assert!(
                func.deprecated.is_none(),
                "math.{name} is standard in {version:?}; no early-warning deprecation"
            );
        }
    }
    // math.log10 stays deprecated in 5.2: its replacement exists there.
    let log10 = lib(LuaVersion::Lua52)
        .lookup(&path(&["math", "log10"]))
        .unwrap();
    if let StdlibEntry::Function(func) = log10 {
        assert!(func.deprecated.is_some());
    }
}

#[test]
fn lua52_math_mod_present_deprecated_like_neighbors() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        let entry = lib(version)
            .lookup(&path(&["math", "mod"]))
            .unwrap_or_else(|| panic!("math.mod compat entry in {version:?}"));
        let StdlibEntry::Function(func) = entry else {
            panic!("math.mod should be a function");
        };
        assert!(
            func.deprecated.is_some(),
            "math.mod deprecated in {version:?}"
        );
    }
}

#[test]
fn lua53_bit32_members_all_deprecated() {
    let lua53 = lib(LuaVersion::Lua53);
    let StdlibEntry::Namespace(namespace) = lua53.lookup(&path(&["bit32"])).unwrap() else {
        panic!("bit32 should be a namespace");
    };
    for (name, entry) in &namespace.members {
        let StdlibEntry::Function(func) = entry else {
            continue;
        };
        assert!(
            func.deprecated.is_some(),
            "bit32.{name} must carry the compat deprecation in 5.3"
        );
    }
}

#[test]
fn randomseed_required_before_54_optional_after() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        let entry = lib(version).lookup(&path(&["math", "randomseed"])).unwrap();
        if let StdlibEntry::Function(func) = entry {
            assert_eq!(func.min_args(), 1, "seed is required in {version:?}");
        }
    }
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let entry = lib(version).lookup(&path(&["math", "randomseed"])).unwrap();
        if let StdlibEntry::Function(func) = entry {
            assert_eq!(func.min_args(), 0, "seed is optional in {version:?}");
            assert_eq!(func.max_args(), Some(2));
        }
    }
}

#[test]
fn lua51_io_lines_filename_only() {
    let entry = lib(LuaVersion::Lua51)
        .lookup(&path(&["io", "lines"]))
        .unwrap();
    if let StdlibEntry::Function(func) = entry {
        assert_eq!(
            func.max_args(),
            Some(1),
            "read formats as io.lines arguments arrived in 5.2"
        );
    }
    let entry_52 = lib(LuaVersion::Lua52)
        .lookup(&path(&["io", "lines"]))
        .unwrap();
    if let StdlibEntry::Function(func) = entry_52 {
        assert_eq!(func.max_args(), None, "5.2+ io.lines accepts read formats");
    }
}

#[test]
fn io_popen_mode_is_r_or_w_only() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let entry = lib(version).lookup(&path(&["io", "popen"])).unwrap();
        let StdlibEntry::Function(func) = entry else {
            panic!("io.popen should be a function");
        };
        let mode = &func.primary_signature().params[1];
        let StdlibArgKind::Constant(values) = &mode.kind else {
            panic!("io.popen mode should be a constant set");
        };
        let mut names: Vec<&str> = values
            .iter()
            .map(|constant| constant.value.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["r", "w"], "io.popen mode set in {version:?}");
    }
}

#[test]
fn lua54_collectgarbage_overloads_and_constant_deprecation() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    let entry = lib(LuaVersion::Lua54)
        .lookup(&path(&["collectgarbage"]))
        .unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("collectgarbage should be a function");
    };
    assert!(
        func.accepts_arg_count(4),
        "collectgarbage('incremental', p, s, sz) is valid in 5.4"
    );
    assert!(!func.accepts_arg_count(5));
    let four_arg: Vec<_> = func.matching_signatures(4).collect();
    assert_eq!(four_arg.len(), 1);
    let StdlibArgKind::Constant(values) = &four_arg[0].params[0].kind else {
        panic!("first arg should be a constant set");
    };
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].value, "incremental");

    let plain = func.primary_signature();
    let StdlibArgKind::Constant(values) = &plain.params[0].kind else {
        panic!("plain options should be a constant set");
    };
    let setpause = values
        .iter()
        .find(|constant| constant.value == "setpause")
        .expect("setpause still callable in 5.4");
    assert!(
        setpause.deprecated.is_some(),
        "5.4 manual 8.2 deprecates the setpause option"
    );
    let collect = values
        .iter()
        .find(|constant| constant.value == "collect")
        .unwrap();
    assert!(collect.deprecated.is_none());
}

#[test]
fn lua55_collectgarbage_param_form_replaces_setpause() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    let entry = lib(LuaVersion::Lua55)
        .lookup(&path(&["collectgarbage"]))
        .unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("collectgarbage should be a function");
    };
    assert!(
        func.accepts_arg_count(3),
        "collectgarbage('param', 'pause', n) is valid in 5.5"
    );
    for signature in &func.signatures {
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
    let three_arg: Vec<_> = func.matching_signatures(3).collect();
    assert_eq!(three_arg.len(), 1);
    let StdlibArgKind::Constant(names) = &three_arg[0].params[1].kind else {
        panic!("param name should be a constant set");
    };
    assert!(names.iter().any(|constant| constant.value == "pause"));
    assert!(names.iter().any(|constant| constant.value == "minormul"));
}

#[test]
fn utf8_lax_flag_only_in_54_plus() {
    for (version, expected_len_max) in [
        (LuaVersion::Lua53, Some(3)),
        (LuaVersion::Lua54, Some(4)),
        (LuaVersion::Lua55, Some(4)),
    ] {
        let entry = lib(version).lookup(&path(&["utf8", "len"])).unwrap();
        if let StdlibEntry::Function(func) = entry {
            assert_eq!(func.max_args(), expected_len_max, "utf8.len in {version:?}");
        }
        let codes = lib(version).lookup(&path(&["utf8", "codes"])).unwrap();
        if let StdlibEntry::Function(func) = codes {
            let expected = if version == LuaVersion::Lua53 { 1 } else { 2 };
            assert_eq!(func.max_args(), Some(expected), "utf8.codes in {version:?}");
        }
    }
}

#[test]
fn arg_global_present_in_all_numbered_versions() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert!(
            lib(version).lookup(&path(&["arg"])).is_some(),
            "the standalone interpreter defines arg in {version:?}"
        );
    }
}

#[test]
fn prompt_globals_follow_manual_documentation() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert!(
            lib(version).lookup(&path(&["_PROMPT"])).is_some(),
            "_PROMPT documented in {version:?}"
        );
        assert!(lib(version).lookup(&path(&["_PROMPT2"])).is_some());
    }
    // The 5.2 manual does not document the prompt overrides.
    assert!(lib(LuaVersion::Lua52).lookup(&path(&["_PROMPT"])).is_none());
}

#[test]
fn lua55_table_create_nrec_is_a_number_hint() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    let entry = lib(LuaVersion::Lua55)
        .lookup(&path(&["table", "create"]))
        .unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("table.create should be a function");
    };
    let nrec = &func.primary_signature().params[1];
    assert!(
        matches!(nrec.kind, StdlibArgKind::Number),
        "5.5 table.create takes a hash-size hint, not a Luau-style fill value"
    );
    assert!(!nrec.required);
}

#[test]
fn lua55_table_create_present_and_frexp_undeprecated() {
    let lua55 = lib(LuaVersion::Lua55);
    assert!(
        lua55.lookup(&path(&["table", "create"])).is_some(),
        "table.create was added in 5.5"
    );
    let frexp = lua55.lookup(&path(&["math", "frexp"])).unwrap();
    if let StdlibEntry::Function(func) = frexp {
        assert!(
            func.deprecated.is_none(),
            "5.5 moved math.frexp back into the standard array"
        );
    }
    let ldexp = lua55.lookup(&path(&["math", "ldexp"])).unwrap();
    if let StdlibEntry::Function(func) = ldexp {
        assert!(
            func.deprecated.is_none(),
            "5.5 moved math.ldexp back into the standard array"
        );
    }
}

#[test]
fn roblox_datatypes_present_with_representative_statics() {
    let roblox_lib = roblox();
    let standalone = lib(LuaVersion::Luau);
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
            roblox_lib.lookup_str(segments).is_some(),
            "{segments:?} must exist in the Roblox library"
        );
        assert!(
            standalone.lookup_str(segments).is_none(),
            "{segments:?} must be absent from standalone Luau"
        );
    }
}

#[test]
fn datetime_has_no_new_constructor() {
    assert!(
        roblox().lookup(&path(&["DateTime", "new"])).is_none(),
        "DateTime only has factory statics; .new is a phantom"
    );
}

#[test]
fn region3_requires_both_corners() {
    let entry = roblox().lookup(&path(&["Region3", "new"])).unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("Region3.new should be a function");
    };
    assert_eq!(func.min_args(), 2);
    assert_eq!(func.max_args(), Some(2));
}

#[test]
fn cframe_new_overloads_accept_documented_arities_only() {
    let entry = roblox().lookup(&path(&["CFrame", "new"])).unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("CFrame.new should be a function");
    };
    for count in [0, 1, 2, 3, 7, 12] {
        assert!(
            func.accepts_arg_count(count),
            "CFrame.new must accept {count} args"
        );
    }
    for count in [4, 5, 6, 8, 11, 13] {
        assert!(
            !func.accepts_arg_count(count),
            "CFrame.new must reject {count} args"
        );
    }
}

#[test]
fn instance_new_parent_param_is_deprecated() {
    let entry = roblox().lookup(&path(&["Instance", "new"])).unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("Instance.new should be a function");
    };
    let parent = &func.primary_signature().params[1];
    assert!(
        parent.deprecated.is_some(),
        "Instance.new's parent arg carries a per-param deprecation"
    );
}

#[test]
fn physical_properties_overloads_match_docs() {
    let entry = roblox()
        .lookup(&path(&["PhysicalProperties", "new"]))
        .unwrap();
    let StdlibEntry::Function(func) = entry else {
        panic!("PhysicalProperties.new should be a function");
    };
    for count in [1, 3, 5, 6] {
        assert!(
            func.accepts_arg_count(count),
            "PhysicalProperties.new must accept {count} args"
        );
    }
    for count in [0, 2, 4, 7] {
        assert!(
            !func.accepts_arg_count(count),
            "PhysicalProperties.new must reject {count} args"
        );
    }
}

fn shape_function<'lib>(
    library: &'lib StdlibLibrary,
    shape: &str,
    name: &str,
) -> &'lib luck_semantic::stdlib_model::StdlibFunction {
    let Some(StdlibEntry::Function(func)) = library.shape_member(shape, name) else {
        panic!(
            "{:?}/{:?}: shape {shape} should have function member {name}",
            library.version, library.environment
        );
    };
    func
}

#[test]
fn file_shape_present_in_all_numbered_versions() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let library = lib(version);
        for member in [
            "read", "lines", "write", "seek", "setvbuf", "close", "flush",
        ] {
            let func = shape_function(library, "file", member);
            assert!(func.is_method, "{version:?}: file:{member} is a method");
        }
        for opener in [
            &["io", "open"][..],
            &["io", "popen"][..],
            &["io", "tmpfile"][..],
            &["io", "input"][..],
            &["io", "output"][..],
        ] {
            let Some(StdlibEntry::Function(func)) = library.lookup_str(opener) else {
                panic!("{version:?}: {opener:?} missing");
            };
            assert_eq!(
                func.returns_shape.as_deref(),
                Some("file"),
                "{version:?}: {opener:?} returns a file"
            );
        }
        for stream in ["stderr", "stdin", "stdout"] {
            let Some(StdlibEntry::Property(value)) = library.lookup_str(&["io", stream]) else {
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
    let lines_51 = shape_function(lib(LuaVersion::Lua51), "file", "lines");
    assert_eq!(lines_51.max_args(), Some(0));
    let write_51 = shape_function(lib(LuaVersion::Lua51), "file", "write");
    assert!(write_51.returns_shape.is_none());
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let lines = shape_function(lib(version), "file", "lines");
        assert_eq!(lines.max_args(), None, "{version:?}: lines takes formats");
        let write = shape_function(lib(version), "file", "write");
        assert_eq!(write.returns_shape.as_deref(), Some("file"), "{version:?}");
    }
}

#[test]
fn string_receiver_shape_derived_in_every_library() {
    let mut libraries: Vec<&StdlibLibrary> = [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ]
    .into_iter()
    .map(lib)
    .collect();
    libraries.push(roblox());
    for library in libraries {
        let upper = shape_function(library, "string", "upper");
        assert!(upper.is_method);
        assert_eq!(upper.min_args(), 0, "subject param dropped");
        assert_eq!(upper.returns_shape.as_deref(), Some("string"));
        let find = shape_function(library, "string", "find");
        assert!(find.min_args() >= 1, "pattern still required");
        for excluded in ["char", "dump"] {
            assert!(
                library.shape_member("string", excluded).is_none(),
                "{:?}/{:?}: string.{excluded} is not subject-first",
                library.version,
                library.environment
            );
        }
    }
}

#[test]
fn derived_rep_arity_follows_source_entry() {
    // rep(s, n) in 5.1 and Luau; rep(s, n, sep) in 5.2+.
    assert_eq!(
        shape_function(lib(LuaVersion::Lua51), "string", "rep").max_args(),
        Some(1)
    );
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            shape_function(lib(version), "string", "rep").max_args(),
            Some(2),
            "{version:?}"
        );
    }
    assert_eq!(
        shape_function(lib(LuaVersion::Luau), "string", "rep").max_args(),
        Some(1)
    );
}

#[test]
fn derived_gfind_method_keeps_deprecation() {
    let gfind = shape_function(lib(LuaVersion::Lua51), "string", "gfind");
    assert!(gfind.deprecated.is_some());
}

#[test]
fn luau_files_have_no_file_shape() {
    assert!(!lib(LuaVersion::Luau).shapes.contains_key("file"));
    assert!(!roblox().shapes.contains_key("file"));
}
// ---- Roblox DataModel, services, and generated class sets ----

mod roblox_api {
    use super::*;
    use luck_semantic::stdlib_model::{StdlibArgKind, StdlibConstant, StdlibFunction};

    fn function<'lib>(lib: &'lib StdlibLibrary, segments: &[&str]) -> &'lib StdlibFunction {
        match lib.lookup_str(segments) {
            Some(StdlibEntry::Function(func)) => func,
            other => panic!("expected function at {segments:?}, got {other:?}"),
        }
    }

    fn constant_set<'lib>(
        lib: &'lib StdlibLibrary,
        segments: &[&str],
        param_idx: usize,
    ) -> &'lib [StdlibConstant] {
        let func = function(lib, segments);
        match &func.primary_signature().params[param_idx].kind {
            StdlibArgKind::Constant(values) => values,
            other => panic!("{segments:?} param {param_idx} not constant: {other:?}"),
        }
    }

    fn find<'a>(values: &'a [StdlibConstant], name: &str) -> Option<&'a StdlibConstant> {
        values.iter().find(|constant| constant.value == name)
    }

    #[test]
    fn game_is_a_shaped_datamodel() {
        let get_service = function(roblox(), &["game", "GetService"]);
        assert!(get_service.is_method);
        assert!(get_service.must_use);
        assert_eq!(get_service.returns_shape.as_deref(), Some("Instance"));
        assert!(roblox().lookup_str(&["game", "FindService"]).is_some());
        assert!(roblox().lookup_str(&["game", "BindToClose"]).is_some());
        assert!(roblox().lookup_str(&["game", "IsLoaded"]).is_some());
    }

    #[test]
    fn datamodel_and_workspace_extend_instance() {
        // Members inherited from the Instance base shape.
        assert!(roblox().lookup_str(&["game", "FindFirstChild"]).is_some());
        assert!(
            roblox()
                .lookup_str(&["workspace", "WaitForChild"])
                .is_some()
        );
        assert!(roblox().lookup_str(&["workspace", "Raycast"]).is_some());
        assert!(
            roblox()
                .lookup_str(&["workspace", "CurrentCamera"])
                .is_some()
        );
        assert!(roblox().lookup_str(&["script", "GetFullName"]).is_some());
        // game.Workspace chains into the Workspace shape.
        assert!(
            roblox()
                .lookup_str(&["game", "Workspace", "Raycast"])
                .is_some()
        );
        // None of this exists standalone.
        assert!(lib(LuaVersion::Luau).lookup_str(&["game"]).is_none());
    }

    #[test]
    fn service_set_sentinels() {
        let services = constant_set(roblox(), &["game", "GetService"], 0);
        assert!(services.len() > 300, "{}", services.len());
        assert!(find(services, "Players").expect("Players").is_common);
        assert!(find(services, "RunService").is_some());
        assert!(find(services, "ProximityPromptService").is_some());
        assert!(
            find(services, "PointsService")
                .expect("PointsService")
                .deprecated
                .is_some(),
            "dead services carry a deprecation marker"
        );
        // Settings-provider children are not game:GetService-able.
        assert!(find(services, "RenderSettings").is_none());
        assert!(find(services, "UserGameSettings").is_none());
    }

    #[test]
    fn class_set_sentinels() {
        let creatable = constant_set(roblox(), &["Instance", "new"], 0);
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
        let is_a = constant_set(roblox(), &["script", "IsA"], 0);
        assert!(
            find(is_a, "BasePart").is_some(),
            "IsA accepts abstract classes"
        );
        assert!(find(is_a, "Instance").is_some());
    }

    #[test]
    fn brickcolor_name_set() {
        let func = function(roblox(), &["BrickColor", "new"]);
        let StdlibArgKind::Constant(names) = &func.signatures[0].params[0].kind else {
            panic!("BrickColor.new arg 0 should be the name set");
        };
        assert_eq!(names.len(), 204);
        assert!(find(names, "Bright red").is_some());
        assert!(find(names, "Really black").is_some());
    }

    #[test]
    fn settings_provider_shapes() {
        let global = roblox()
            .shape_member("GlobalSettings", "GetService")
            .expect("GlobalSettings:GetService");
        let StdlibEntry::Function(func) = global else {
            panic!("expected function");
        };
        let StdlibArgKind::Constant(values) = &func.primary_signature().params[0].kind else {
            panic!("expected constant set");
        };
        assert!(find(values, "Studio").is_some());
        assert!(find(values, "RenderSettings").is_some());
        let user = roblox()
            .shape_member("UserSettings", "GetService")
            .expect("UserSettings:GetService");
        let StdlibEntry::Function(func) = user else {
            panic!("expected function");
        };
        let StdlibArgKind::Constant(values) = &func.primary_signature().params[0].kind else {
            panic!("expected constant set");
        };
        assert!(find(values, "UserGameSettings").is_some());
    }

    fn analysis(source: &str) -> (luck_parser::ParseResult, luck_semantic::SemanticAnalysis) {
        let parsed = luck_parser::parse(source, LuaVersion::Luau);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let semantic = luck_semantic::analyze_with_environment(
            &parsed.block,
            LuaVersion::Luau,
            StdlibEnvironment::Roblox,
        );
        (parsed, semantic)
    }

    /// The last statement's call - either a call statement or the last
    /// initializer of a local assignment.
    fn last_call(parsed: &luck_parser::ParseResult) -> &luck_ast::expr::FunctionCall {
        match parsed.block.stmts.last().expect("statement") {
            luck_ast::Statement::FunctionCall(stmt) => &stmt.call,
            luck_ast::Statement::LocalAssignment(local) => {
                match local.exprs.as_ref().and_then(|exprs| exprs.iter().last()) {
                    Some(luck_ast::Expression::FunctionCall(call)) => call,
                    other => panic!("expected call initializer, got {other:?}"),
                }
            }
            other => panic!("expected call statement, got {other:?}"),
        }
    }

    #[test]
    fn get_service_call_resolves() {
        let (parsed, semantic) = analysis("game:GetService('Players')");
        let (name, resolved) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("game:GetService resolves");
        assert_eq!(name, "game:GetService");
        assert!(resolved.is_method_call);
    }

    #[test]
    fn get_service_chain_resolves() {
        let (parsed, semantic) = analysis(
            "local players = game:GetService('Players')\nlocal x = players:FindFirstChild('x')",
        );
        let (name, _) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("chained FindFirstChild resolves");
        assert_eq!(name, "players:FindFirstChild");
    }

    #[test]
    fn script_parent_chain_resolves() {
        let (parsed, semantic) =
            analysis("local container = script.Parent\nlocal x = container:WaitForChild('Config')");
        let (name, _) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("script.Parent chain resolves");
        assert_eq!(name, "container:WaitForChild");
    }

    #[test]
    fn user_settings_call_chain_resolves() {
        let (parsed, semantic) = analysis("UserSettings():GetService('UserGameSettings')");
        let (name, _) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("UserSettings():GetService resolves");
        assert_eq!(name, "UserSettings:GetService");
    }

    #[test]
    fn workspace_raycast_resolves() {
        let (parsed, semantic) = analysis("workspace:Raycast(origin, direction)");
        let (name, _) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("workspace:Raycast resolves");
        assert_eq!(name, "workspace:Raycast");
    }

    #[test]
    fn enum_get_enum_items_resolves() {
        let (parsed, semantic) = analysis("local items = Enum.Material:GetEnumItems()");
        let (name, resolved) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("Enum.Material:GetEnumItems resolves");
        assert_eq!(name, "Enum:GetEnumItems");
        assert!(resolved.is_method_call);
    }

    #[test]
    fn enum_from_name_resolves() {
        let (parsed, semantic) = analysis("local item = Enum.Material:FromName('Grass')");
        semantic
            .resolve_callee(last_call(&parsed))
            .expect("Enum.Material:FromName resolves");
    }

    #[test]
    fn enum_global_get_enums_resolves() {
        let (parsed, semantic) = analysis("local all = Enum:GetEnums()");
        let (name, _) = semantic
            .resolve_callee(last_call(&parsed))
            .expect("Enum:GetEnums resolves");
        assert_eq!(name, "Enum:GetEnums");
    }

    #[test]
    fn enum_item_name_is_string_shaped() {
        // EnumItem.Name carries the string shape, so string methods
        // chain off it.
        let (parsed, semantic) = analysis("local upper = Enum.Material.Grass.Name:upper()");
        semantic
            .resolve_callee(last_call(&parsed))
            .expect("Name:upper() resolves through the string shape");
    }
}

// ---- generated Enum tree ----

mod enum_tree {
    use super::*;

    #[test]
    fn enum_tree_is_populated() {
        let StdlibEntry::Namespace(enum_global) = roblox()
            .lookup_str(&["Enum"])
            .expect("Enum global on Roblox")
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
            let entry = roblox()
                .lookup_str(path)
                .unwrap_or_else(|| panic!("missing {path:?}"));
            let StdlibEntry::Constant(value) = entry else {
                panic!("{path:?} should be a constant");
            };
            assert!(value.read_only);
            assert_eq!(value.shape.as_deref(), Some("EnumItem"));
        }
    }

    #[test]
    fn enum_types_are_enum_shaped() {
        let StdlibEntry::Namespace(material) = roblox()
            .lookup_str(&["Enum", "Material"])
            .expect("Enum.Material")
        else {
            panic!("Enum.Material should be a namespace");
        };
        assert_eq!(material.shape.as_deref(), Some("Enum"));
    }

    #[test]
    fn deprecated_item_carries_marker() {
        let entry = roblox()
            .lookup_str(&["Enum", "AlignType", "Parallel"])
            .expect("Enum.AlignType.Parallel");
        assert!(
            entry.deprecation().is_some(),
            "AlignType.Parallel is tagged Deprecated in the dump"
        );
    }

    #[test]
    fn item_properties_reachable_through_shape() {
        for member in ["Name", "Value", "EnumType"] {
            assert!(
                roblox()
                    .lookup_str(&["Enum", "Material", "Grass", member])
                    .is_some(),
                "EnumItem member {member} should resolve through the shape"
            );
        }
    }

    #[test]
    fn standalone_luau_has_no_enum() {
        assert!(lib(LuaVersion::Luau).lookup_str(&["Enum"]).is_none());
    }
}

// ---- generated roblox_api.toml regen pipeline ----

mod regen {
    use std::path::PathBuf;

    const DUMP_URL: &str =
        "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/API-Dump.json";

    /// GetService sets of the settings ServiceProviders (settings() /
    /// UserSettings()); tagged [Service] in the dump but not reachable
    /// through game:GetService.
    const SETTINGS_CHILDREN: &[&str] = &[
        "DebugSettings",
        "GameSettings",
        "LuaSettings",
        "NetworkSettings",
        "PhysicsSettings",
        "RenderSettings",
        "Studio",
        "UserGameSettings",
    ];

    /// Curated frequently-used services, ranked first by completion.
    const COMMON_SERVICES: &[&str] = &[
        "AnalyticsService",
        "AssetService",
        "AvatarEditorService",
        "BadgeService",
        "CaptureService",
        "ChangeHistoryService",
        "Chat",
        "CollectionService",
        "ContentProvider",
        "ContextActionService",
        "CoreGui",
        "DataStoreService",
        "Debris",
        "ExperienceNotificationService",
        "GamepadService",
        "GeometryService",
        "GroupService",
        "GuiService",
        "HapticService",
        "HttpService",
        "InsertService",
        "KeyframeSequenceProvider",
        "Lighting",
        "LocalizationService",
        "LogService",
        "MarketplaceService",
        "MaterialService",
        "MemoryStoreService",
        "MessagingService",
        "PathfindingService",
        "PhysicsService",
        "Players",
        "PolicyService",
        "ProximityPromptService",
        "ReplicatedFirst",
        "ReplicatedStorage",
        "RunService",
        "ScriptContext",
        "Selection",
        "ServerScriptService",
        "ServerStorage",
        "SocialService",
        "SoundService",
        "StarterGui",
        "StarterPack",
        "StarterPlayer",
        "Stats",
        "StudioService",
        "Teams",
        "TeleportService",
        "TestService",
        "TextChatService",
        "TextService",
        "TweenService",
        "UserInputService",
        "UserService",
        "VRService",
        "VoiceChatService",
        "Workspace",
    ];

    fn generated_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/roblox_api.toml")
    }

    fn generated_enums_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/roblox_enums.toml")
    }

    fn value_line(name: &str, deprecated_msg: Option<&str>, common: bool) -> String {
        let mut attrs: Vec<String> = Vec::new();
        if common {
            attrs.push("common = true".to_string());
        }
        if let Some(msg) = deprecated_msg {
            attrs.push(format!("deprecated = {{ message = \"{msg}\" }}"));
        }
        if attrs.is_empty() {
            format!("  \"{name}\",")
        } else {
            format!("  {{ value = \"{name}\", {} }},", attrs.join(", "))
        }
    }

    #[test]
    #[ignore = "fetches the live Roblox API dump and rewrites roblox_api.toml; run with --ignored to refresh"]
    fn regenerate_roblox_api() {
        let body = ureq::get(DUMP_URL)
            .call()
            .expect("fetch Roblox API dump")
            .body_mut()
            .read_to_string()
            .expect("read API dump body");
        let dump: serde_json::Value = serde_json::from_str(&body).expect("parse API dump JSON");
        let classes = dump["Classes"].as_array().expect("Classes array");
        let mut rows: Vec<(String, bool, bool, bool)> = classes
            .iter()
            .map(|class| {
                let name = class["Name"].as_str().expect("class Name").to_string();
                let tags: Vec<&str> = class["Tags"]
                    .as_array()
                    .map(|tags| tags.iter().filter_map(|tag| tag.as_str()).collect())
                    .unwrap_or_default();
                (
                    name,
                    tags.contains(&"Service"),
                    !tags.contains(&"NotCreatable"),
                    tags.contains(&"Deprecated"),
                )
            })
            .collect();
        rows.sort();

        let mut services: Vec<String> = Vec::new();
        let mut creatable: Vec<String> = Vec::new();
        let mut all_classes: Vec<String> = Vec::new();
        for (name, is_service, is_creatable, is_deprecated) in &rows {
            let class_msg = is_deprecated.then_some("deprecated Roblox class");
            if *is_service && !SETTINGS_CHILDREN.contains(&name.as_str()) {
                services.push(value_line(
                    name,
                    is_deprecated.then_some("deprecated Roblox service"),
                    COMMON_SERVICES.contains(&name.as_str()),
                ));
            }
            if *is_creatable {
                creatable.push(value_line(name, class_msg, false));
            }
            all_classes.push(value_line(name, class_msg, false));
        }
        for common in COMMON_SERVICES {
            assert!(
                rows.iter()
                    .any(|(name, is_service, ..)| name == common && *is_service),
                "curated common service {common} not in the dump"
            );
        }

        let mut out = String::new();
        out.push_str("# GENERATED FILE - do not hand-edit.\n");
        out.push_str(
            "# Regenerate: cargo test -p luck_semantic regenerate_roblox_api -- --ignored\n",
        );
        out.push_str(
            "# Source: Roblox API dump, MaximumADHD/Roblox-Client-Tracker (roblox branch).\n",
        );
        out.push_str("# Spliced into the luau_roblox library; luau_roblox.toml params reference\n");
        out.push_str("# these sets by name (see stdlib_model.rs for the constant_sets schema).\n");
        out.push_str("# Settings-provider classes (settings() / UserSettings() children) are\n");
        out.push_str(
            "# excluded from roblox_services: they are not reachable via game:GetService.\n\n",
        );
        out.push_str("[constant_sets.roblox_services]\nvalues = [\n");
        out.push_str(&services.join("\n"));
        out.push_str("\n]\n\n[constant_sets.roblox_creatable_classes]\nvalues = [\n");
        out.push_str(&creatable.join("\n"));
        out.push_str("\n]\n\n[constant_sets.roblox_all_classes]\nvalues = [\n");
        out.push_str(&all_classes.join("\n"));
        out.push_str("\n]\n");

        std::fs::write(generated_path(), out).expect("write roblox_api.toml");
        write_enum_file(&dump);
    }

    /// The `[enums]` companion artifact: every enum type and item from
    /// the dump, deprecation tags carried over. Same command, second
    /// file - see stdlib_model.rs `splice_enums` for the loader side.
    fn write_enum_file(dump: &serde_json::Value) {
        let enums = dump["Enums"].as_array().expect("Enums array");
        type EnumRow = (String, bool, Vec<(String, bool)>);
        let mut types: Vec<EnumRow> = enums
            .iter()
            .map(|enum_type| {
                let name = enum_type["Name"].as_str().expect("enum Name").to_string();
                let deprecated = has_deprecated_tag(enum_type);
                let mut items: Vec<(String, bool)> = enum_type["Items"]
                    .as_array()
                    .expect("enum Items")
                    .iter()
                    .map(|item| {
                        (
                            item["Name"].as_str().expect("item Name").to_string(),
                            has_deprecated_tag(item),
                        )
                    })
                    .collect();
                items.sort();
                items.dedup_by(|a, b| a.0 == b.0);
                (name, deprecated, items)
            })
            .collect();
        types.sort();

        let mut out = String::new();
        out.push_str("# GENERATED FILE - do not hand-edit.\n");
        out.push_str(
            "# Regenerate: cargo test -p luck_semantic regenerate_roblox_api -- --ignored\n",
        );
        out.push_str(
            "# Source: Roblox API dump, MaximumADHD/Roblox-Client-Tracker (roblox branch).\n",
        );
        out.push_str("# Spliced into the luau_roblox library: each [enums.<Type>] becomes an\n");
        out.push_str("# Enum-shaped member namespace of the Enum global, each item an\n");
        out.push_str("# EnumItem-shaped read-only constant (see stdlib_model.rs splice_enums).\n");
        for (name, deprecated, items) in &types {
            out.push_str(&format!("\n[enums.{name}]\n"));
            if *deprecated {
                out.push_str("deprecated = { message = \"deprecated Roblox enum\" }\n");
            }
            out.push_str("items = [\n");
            for (item, item_deprecated) in items {
                if *item_deprecated {
                    out.push_str(&format!(
                        "  {{ value = \"{item}\", deprecated = {{ message = \"deprecated Roblox enum item\" }} }},\n"
                    ));
                } else {
                    out.push_str(&format!("  \"{item}\",\n"));
                }
            }
            out.push_str("]\n");
        }

        std::fs::write(generated_enums_path(), out).expect("write roblox_enums.toml");
    }

    fn has_deprecated_tag(node: &serde_json::Value) -> bool {
        node["Tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some("Deprecated")))
    }

    /// The committed enum artifact stays sane without network access:
    /// generated header, populated, and pinned sentinels intact.
    #[test]
    fn committed_roblox_enums_file_is_plausible() {
        let committed = std::fs::read_to_string(generated_enums_path())
            .expect("read committed roblox_enums.toml");
        assert!(committed.starts_with("# GENERATED FILE"));
        for sentinel in [
            "[enums.Material]",
            "[enums.KeyCode]",
            "[enums.EasingStyle]",
            "{ value = \"Parallel\", deprecated",
        ] {
            assert!(committed.contains(sentinel), "missing {sentinel}");
        }
    }

    /// The committed generated file stays sane without network access:
    /// it must parse (implicit in every `roblox()` test) and keep its
    /// three sets non-trivially populated.
    #[test]
    fn committed_roblox_api_file_is_plausible() {
        let committed =
            std::fs::read_to_string(generated_path()).expect("read committed roblox_api.toml");
        assert!(committed.starts_with("# GENERATED FILE"));
        for set in [
            "[constant_sets.roblox_services]",
            "[constant_sets.roblox_creatable_classes]",
            "[constant_sets.roblox_all_classes]",
        ] {
            assert!(committed.contains(set), "missing {set}");
        }
    }
}
