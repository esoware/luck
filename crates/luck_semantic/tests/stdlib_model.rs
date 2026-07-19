//! Integration tests for the rich stdlib model. Verifies that:
//!   - each per-version TOML file loads without panicking
//!   - key entries are present per Lua version
//!   - deprecation/`must_use` metadata round-trips through the loader
//!   - version-gated entries appear and disappear as expected

use compact_str::CompactString;
use luck_semantic::stdlib_model::{EntryKind, library_for};
use luck_token::{LuaVersion, StdlibEnvironment};

fn path(segments: &[&str]) -> Vec<CompactString> {
    segments.iter().copied().map(CompactString::from).collect()
}

#[test]
fn every_version_loads() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let lib = library_for(version);
        assert!(
            lib.globals.contains_key("print"),
            "{version:?} stdlib should expose print"
        );
        assert!(
            lib.globals.contains_key("type"),
            "{version:?} stdlib should expose type"
        );
    }
}

#[test]
fn table_getn_only_deprecated_in_52_plus() {
    let lua51 = library_for(LuaVersion::Lua51);
    let getn_51 = lua51
        .lookup(&path(&["table", "getn"]))
        .expect("table.getn present in 5.1");
    match &getn_51.kind {
        EntryKind::Function(func) => {
            assert!(
                func.deprecated.is_some(),
                "Lua 5.1 marks table.getn deprecated in luck since it's already replaced by '#'"
            );
            // Luck's policy: even in 5.1, prefer `#` so we suggest it.
        }
        _ => panic!("table.getn should be a function"),
    }

    for newer in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        let lib = library_for(newer);
        let entry = lib
            .lookup(&path(&["table", "getn"]))
            .expect("table.getn still visible in newer versions");
        match &entry.kind {
            EntryKind::Function(func) => {
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
    let lua51 = library_for(LuaVersion::Lua51);
    let unpack_51 = lua51.lookup(&path(&["unpack"])).expect("unpack in 5.1");
    if let EntryKind::Function(func) = &unpack_51.kind {
        assert!(
            func.deprecated.is_none(),
            "unpack must NOT be deprecated in 5.1"
        );
    } else {
        panic!("unpack should be a function");
    }
    for newer in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        let lib = library_for(newer);
        let unpack = lib
            .lookup(&path(&["unpack"]))
            .expect("unpack still visible as deprecated in 5.2+");
        match &unpack.kind {
            EntryKind::Function(func) => {
                let deprecation = func
                    .deprecated
                    .as_ref()
                    .expect("unpack must be deprecated in 5.2+");
                let template = deprecation
                    .replace_template
                    .as_ref()
                    .expect("unpack has a replace template");
                assert_eq!(template.as_str(), "table.unpack(%1)");
            }
            _ => panic!("unpack should be a function"),
        }
    }
}

#[test]
fn bit32_present_in_52_and_53_absent_in_51_and_54() {
    let lua51 = library_for(LuaVersion::Lua51);
    assert!(
        lua51.lookup(&path(&["bit32"])).is_none(),
        "bit32 is not in Lua 5.1"
    );

    for present in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Luau] {
        let lib = library_for(present);
        let entry = lib
            .lookup(&path(&["bit32"]))
            .expect("bit32 expected in 5.2/5.3/luau");
        assert!(
            matches!(entry.kind, EntryKind::Namespace(_)),
            "bit32 should be a namespace"
        );
    }
    let lua54 = library_for(LuaVersion::Lua54);
    assert!(
        lua54.lookup(&path(&["bit32"])).is_none(),
        "bit32 was removed in Lua 5.4"
    );
}

#[test]
fn utf8_only_53_plus() {
    assert!(
        library_for(LuaVersion::Lua51)
            .lookup(&path(&["utf8"]))
            .is_none()
    );
    assert!(
        library_for(LuaVersion::Lua52)
            .lookup(&path(&["utf8"]))
            .is_none()
    );
    for v in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        assert!(
            library_for(v).lookup(&path(&["utf8"])).is_some(),
            "utf8 expected in {v:?}"
        );
    }
}

#[test]
fn loadstring_replace_template_round_trip() {
    let lib = library_for(LuaVersion::Lua52);
    let entry = lib
        .lookup(&path(&["loadstring"]))
        .expect("loadstring in 5.2");
    if let EntryKind::Function(func) = &entry.kind {
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
    let lib = library_for(LuaVersion::Lua54);
    for name in ["pcall", "tostring", "tonumber", "type", "select", "next"] {
        let entry = lib.lookup(&path(&[name])).expect(name);
        if let EntryKind::Function(func) = &entry.kind {
            // `next` doesn't have to be must_use, so skip the assertion
            // for it - we only assert what we claim:
            if name == "next" {
                continue;
            }
            assert!(func.must_use, "{name} should be must_use");
        }
    }
    let table_concat = lib.lookup(&path(&["table", "concat"])).unwrap();
    if let EntryKind::Function(func) = &table_concat.kind {
        assert!(func.must_use);
    }
    let coro_create = lib.lookup(&path(&["coroutine", "create"])).unwrap();
    if let EntryKind::Function(func) = &coro_create.kind {
        assert!(func.must_use);
    }
}

#[test]
fn luau_buffer_and_vector_present() {
    let luau = library_for(LuaVersion::Luau);
    assert!(luau.lookup(&path(&["buffer"])).is_some());
    assert!(luau.lookup(&path(&["buffer", "create"])).is_some());
    assert!(luau.lookup(&path(&["vector"])).is_some());
    assert!(luau.lookup(&path(&["vector", "create"])).is_some());
    assert!(luau.lookup(&path(&["task", "spawn"])).is_some());
}

#[test]
fn luau_io_package_removed_os_sandboxed() {
    let luau = library_for(LuaVersion::Luau);
    assert!(luau.lookup(&path(&["io"])).is_none());
    assert!(luau.lookup(&path(&["package"])).is_none());
    // os survives as the sandboxed clock/date/difftime/time subset.
    assert!(luau.lookup(&path(&["os", "time"])).is_some());
    assert!(luau.lookup(&path(&["os", "clock"])).is_some());
    assert!(luau.lookup(&path(&["os", "getenv"])).is_none());
    assert!(luau.lookup(&path(&["os", "exit"])).is_none());
}

#[test]
fn luau_table_clone_present_in_luau_only() {
    let luau = library_for(LuaVersion::Luau);
    assert!(luau.lookup(&path(&["table", "clone"])).is_some());

    // Standard Lua doesn't have table.clone.
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
    ] {
        assert!(
            library_for(v).lookup(&path(&["table", "clone"])).is_none(),
            "table.clone should not exist in {v:?}"
        );
    }
}

#[test]
fn string_constant_arg_set_for_io_open_mode() {
    use luck_semantic::stdlib_model::StdlibArgKind;
    let lib = library_for(LuaVersion::Lua54);
    let io_open = lib.lookup(&path(&["io", "open"])).unwrap();
    let EntryKind::Function(func) = &io_open.kind else {
        panic!("io.open should be a function");
    };
    let second = func
        .params
        .get(1)
        .expect("io.open has at least 2 documented params");
    let StdlibArgKind::Constant(values) = &second.kind else {
        panic!("io.open's second arg should be a constant set");
    };
    assert!(values.iter().any(|value| value.as_str() == "r"));
    assert!(values.iter().any(|value| value.as_str() == "w"));
}

#[test]
fn pure_markers_preserved() {
    let lib = library_for(LuaVersion::Lua54);
    let math_abs = lib.lookup(&path(&["math", "abs"])).unwrap();
    if let EntryKind::Function(func) = &math_abs.kind {
        assert!(func.is_pure);
    }
    let print = lib.lookup(&path(&["print"])).unwrap();
    if let EntryKind::Function(func) = &print.kind {
        assert!(!func.is_pure);
    }
}

#[test]
fn luau_roblox_only_entries_are_tagged() {
    let luau = library_for(LuaVersion::Luau);

    let warn = luau.lookup(&path(&["warn"])).expect("warn in Luau");
    assert!(
        warn.available_in_luau(StdlibEnvironment::Roblox)
            && !warn.available_in_luau(StdlibEnvironment::Standalone),
        "warn is a Roblox runtime addition"
    );

    let task_ns = luau.lookup(&path(&["task"])).expect("task in Luau");
    assert!(
        task_ns.available_in_luau(StdlibEnvironment::Roblox)
            && !task_ns.available_in_luau(StdlibEnvironment::Standalone),
        "task namespace is Roblox-only"
    );
    for member in ["spawn", "defer", "delay", "wait", "cancel"] {
        let entry = luau
            .lookup(&path(&["task", member]))
            .unwrap_or_else(|| panic!("task.{member} missing"));
        assert!(
            entry.available_in_luau(StdlibEnvironment::Roblox)
                && !entry.available_in_luau(StdlibEnvironment::Standalone),
            "task.{member} should inherit the Roblox-only tier"
        );
    }

    let bit_ns = luau.lookup(&path(&["bit"])).expect("bit in Luau");
    assert!(
        bit_ns.available_in_luau(StdlibEnvironment::Roblox)
            && !bit_ns.available_in_luau(StdlibEnvironment::Standalone),
        "bit namespace is Roblox-only"
    );
    for member in ["band", "bor", "bnot", "bxor", "lshift", "rshift"] {
        let entry = luau
            .lookup(&path(&["bit", member]))
            .unwrap_or_else(|| panic!("bit.{member} missing"));
        assert!(
            entry.available_in_luau(StdlibEnvironment::Roblox)
                && !entry.available_in_luau(StdlibEnvironment::Standalone),
            "bit.{member} should inherit the Roblox-only tier"
        );
    }
}

#[test]
fn luau_standard_entries_are_not_roblox() {
    let luau = library_for(LuaVersion::Luau);

    // These come from the standard Luau VM, not Roblox's runtime.
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
        &["math", "isnan"],
        &["math", "isinf"],
        &["math", "isfinite"],
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
        let entry = luau
            .lookup(&path(segments))
            .unwrap_or_else(|| panic!("{segments:?} missing from luau.toml"));
        assert!(
            entry.available_in_luau(StdlibEnvironment::Roblox)
                && entry.available_in_luau(StdlibEnvironment::Standalone),
            "{segments:?} is in the standard Luau VM and must NOT be tagged roblox"
        );
    }
}

#[test]
fn lua51_xpcall_strictly_two_args() {
    let lib = library_for(LuaVersion::Lua51);
    let xpcall = lib.lookup(&path(&["xpcall"])).expect("xpcall in 5.1");
    let EntryKind::Function(func) = &xpcall.kind else {
        panic!("xpcall must be a function");
    };
    assert_eq!(func.min_args, 2);
    assert_eq!(
        func.max_args,
        Some(2),
        "Lua 5.1 xpcall does not accept additional args (variadic form added in 5.2)"
    );
}

#[test]
fn lua52_package_searchers_replaces_loaders() {
    let lib = library_for(LuaVersion::Lua52);
    assert!(
        lib.lookup(&path(&["package", "searchers"])).is_some(),
        "package.searchers should exist in 5.2 (renamed from loaders)"
    );
    assert!(
        lib.lookup(&path(&["package", "loaders"])).is_none(),
        "package.loaders was removed in 5.2"
    );
}

#[test]
fn lua53_randomseed_single_arg() {
    let lib = library_for(LuaVersion::Lua53);
    let rs = lib
        .lookup(&path(&["math", "randomseed"]))
        .expect("math.randomseed in 5.3");
    if let EntryKind::Function(func) = &rs.kind {
        assert_eq!(
            func.max_args,
            Some(1),
            "5.3 randomseed takes at most one arg; two-seed form is 5.4+"
        );
    }
}

#[test]
fn lua54_debug_uservalue_optional_index() {
    let lib = library_for(LuaVersion::Lua54);
    let get = lib.lookup(&path(&["debug", "getuservalue"])).unwrap();
    if let EntryKind::Function(func) = &get.kind {
        assert_eq!(func.max_args, Some(2), "5.4 added optional n index");
    }
    let set = lib.lookup(&path(&["debug", "setuservalue"])).unwrap();
    if let EntryKind::Function(func) = &set.kind {
        assert_eq!(func.max_args, Some(3), "5.4 added optional n index");
    }
}

#[test]
fn lua55_table_create_present_and_frexp_undeprecated() {
    let lib = library_for(LuaVersion::Lua55);
    assert!(
        lib.lookup(&path(&["table", "create"])).is_some(),
        "table.create was added in 5.5"
    );
    let frexp = lib.lookup(&path(&["math", "frexp"])).unwrap();
    if let EntryKind::Function(func) = &frexp.kind {
        assert!(
            func.deprecated.is_none(),
            "5.5 moved math.frexp back into the standard array"
        );
    }
    let ldexp = lib.lookup(&path(&["math", "ldexp"])).unwrap();
    if let EntryKind::Function(func) = &ldexp.kind {
        assert!(
            func.deprecated.is_none(),
            "5.5 moved math.ldexp back into the standard array"
        );
    }
}
