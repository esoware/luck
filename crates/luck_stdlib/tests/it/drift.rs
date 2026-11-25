//! The seven files don't inherit from each other, so copies of a shared
//! entry can drift apart.
//!
//! The two Luau files must agree on every shared
//! entry, and so must each pair of adjacent Lua versions, unless an
//! allowlist below names the difference and the reason for it. The other
//! tests check each library's internal consistency.

use std::collections::{BTreeMap, BTreeSet};

use luck_stdlib::{
    StdlibArgKind, StdlibDeprecation, StdlibEntry, StdlibLibrary, StdlibParam, StdlibSignature,
};
use luck_token::LuaVersion;

use crate::common::{NUMBERED_VERSIONS, all_libraries, luau, roblox, standalone_library};

fn global_paths(
    library: &'static StdlibLibrary<'static>,
) -> BTreeMap<String, &'static StdlibEntry<'static>> {
    let mut paths = BTreeMap::new();
    for (name, entry) in &library.globals {
        collect(name, entry, &mut paths);
    }
    paths
}

fn shape_paths(
    library: &'static StdlibLibrary<'static>,
) -> BTreeMap<String, &'static StdlibEntry<'static>> {
    let mut paths = BTreeMap::new();
    for (shape_name, shape) in &library.shapes {
        for (member, entry) in &shape.members {
            collect(&format!("{shape_name}:{member}"), entry, &mut paths);
        }
    }
    paths
}

fn all_entries(
    library: &'static StdlibLibrary<'static>,
) -> BTreeMap<String, &'static StdlibEntry<'static>> {
    let mut entries = global_paths(library);
    entries.extend(shape_paths(library));
    entries
}

fn collect(
    path: &str,
    entry: &'static StdlibEntry<'static>,
    paths: &mut BTreeMap<String, &'static StdlibEntry<'static>>,
) {
    paths.insert(path.to_owned(), entry);
    if let StdlibEntry::Namespace(namespace) = entry {
        for (name, child) in &namespace.members {
            collect(&format!("{path}.{name}"), child, paths);
        }
    }
}

fn param_fingerprint(param: &StdlibParam<'_>) -> String {
    let kind = match &param.kind {
        StdlibArgKind::Any => "any".to_owned(),
        StdlibArgKind::Bool => "bool".to_owned(),
        StdlibArgKind::Number => "number".to_owned(),
        StdlibArgKind::String => "string".to_owned(),
        StdlibArgKind::Function => "function".to_owned(),
        StdlibArgKind::Table => "table".to_owned(),
        StdlibArgKind::Vararg => "...".to_owned(),
        StdlibArgKind::Display(label) => format!("<{label}>"),
        StdlibArgKind::Constant(values) => {
            let rendered: Vec<&str> = values.iter().map(|constant| constant.value).collect();
            format!("constant{{{}}}", rendered.join(","))
        }
    };
    format!(
        "{kind}{}{},",
        if param.is_required { "" } else { "?" },
        if param.deprecated.is_some() { "!" } else { "" }
    )
}

fn signature_fingerprint(signature: &StdlibSignature<'_>) -> String {
    let max_args = signature.max_args.map_or_else(|| "*".to_owned(), |max| max.to_string());
    let params: String = signature.params.iter().map(param_fingerprint).collect();
    format!(" [{}..{max_args}]({params})", signature.min_args)
}

/// Deprecation has its own fingerprint, since it legitimately changes as
/// APIs age.
fn fingerprint(entry: &StdlibEntry<'_>) -> String {
    match entry {
        StdlibEntry::Function(function) => {
            let mut output = String::from("fn");
            if function.is_pure {
                output.push_str(" pure");
            }
            if function.is_must_use {
                output.push_str(" must_use");
            }
            if function.is_method {
                output.push_str(" method");
            }
            if !function.is_read_only {
                output.push_str(" writable");
            }
            if let Some(shape) = &function.returns_shape {
                output.push_str(" -> ");
                output.push_str(shape);
            }
            output.extend(function.signatures.iter().map(signature_fingerprint));
            output
        }
        StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
            let kind = if matches!(entry, StdlibEntry::Constant(_)) { "const" } else { "prop" };
            format!(
                "{kind}{}{}",
                if value.is_read_only { "" } else { " writable" },
                value.shape.as_ref().map_or(String::new(), |shape| format!(" : {shape}"))
            )
        }
        StdlibEntry::Namespace(namespace) => format!(
            "namespace{}",
            namespace.shape.as_ref().map_or(String::new(), |shape| format!(" : {shape}"))
        ),
    }
}

fn deprecation_fingerprint(entry: &StdlibEntry<'_>) -> String {
    let Some(deprecation) = entry.deprecation() else {
        return "live".to_owned();
    };
    let replacement = match deprecation.replacement {
        Some(replacement) => format!("{} exact={}", replacement.template, replacement.is_exact),
        None => "-".to_owned(),
    };
    format!("deprecated({}|{replacement})", deprecation.message)
}

#[test]
fn luau_and_roblox_shared_surface_agrees() {
    // Signature divergence: Roblox vectors are 3-wide, while standalone Luau
    // also has the 4-arg form of LUA_VECTOR_SIZE=4 builds. And the standalone
    // CLI's collectgarbage also accepts "collect", which returns nothing.
    const SIGNATURE_ALLOWLIST: [&str; 2] = ["vector.create", "collectgarbage"];
    // Deprecation divergence: Roblox deprecates these while they stay fully
    // supported in standalone Luau.
    const DEPRECATION_ALLOWLIST: [&str; 3] = ["collectgarbage", "getfenv", "setfenv"];

    let standalone_globals = global_paths(luau());
    let roblox_globals = global_paths(roblox());
    let mut mismatches = Vec::new();
    for (path, standalone_entry) in &standalone_globals {
        let Some(roblox_entry) = roblox_globals.get(path) else {
            continue;
        };
        let standalone_fingerprint = fingerprint(standalone_entry);
        let roblox_fingerprint = fingerprint(roblox_entry);
        if standalone_fingerprint != roblox_fingerprint
            && !SIGNATURE_ALLOWLIST.contains(&path.as_str())
        {
            mismatches.push(format!(
                "{path}:\n  standalone: {standalone_fingerprint}\n  roblox:     {roblox_fingerprint}"
            ));
        }
        let standalone_deprecation = deprecation_fingerprint(standalone_entry);
        let roblox_deprecation = deprecation_fingerprint(roblox_entry);
        if standalone_deprecation != roblox_deprecation
            && !DEPRECATION_ALLOWLIST.contains(&path.as_str())
        {
            mismatches.push(format!(
                "{path} deprecation:\n  standalone: {standalone_deprecation}\n  roblox:     {roblox_deprecation}"
            ));
        }
    }

    // The derived string receiver is the only shape both define. It mirrors
    // the namespaces compared above, so this only catches a hand-authored
    // override in one file.
    let standalone_shapes = shape_paths(luau());
    let roblox_shapes = shape_paths(roblox());
    for (path, standalone_entry) in &standalone_shapes {
        let Some(roblox_entry) = roblox_shapes.get(path) else {
            continue;
        };
        let standalone_fingerprint = fingerprint(standalone_entry);
        let roblox_fingerprint = fingerprint(roblox_entry);
        if standalone_fingerprint != roblox_fingerprint {
            mismatches.push(format!(
                "shape {path}:\n  standalone: {standalone_fingerprint}\n  roblox:     {roblox_fingerprint}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "luau.toml and luau_roblox.toml drifted on shared entries:\n{}",
        mismatches.join("\n")
    );
}

/// Each allowlist entry cites its manual change. Deprecation isn't
/// compared, since APIs routinely age into it and its policy has its own
/// tests.
#[test]
fn numbered_lua_adjacent_versions_agree() {
    // (path, older version of the adjacent pair) -> manual delta.
    let allowlist: &[(&str, LuaVersion)] = &[
        // 5.2 load accepts a chunk string or function plus mode/env;
        // 5.1 load takes only a function.
        ("load", LuaVersion::Lua51),
        // 5.2 loadfile gains mode and env parameters.
        ("loadfile", LuaVersion::Lua51),
        // 5.2 xpcall becomes variadic (extra args forwarded to f).
        ("xpcall", LuaVersion::Lua51),
        // 5.2 os.exit accepts true/false as well as a number, plus the
        // close flag.
        ("os.exit", LuaVersion::Lua51),
        // 5.2 io.lines forwards read formats to the iterator.
        ("io.lines", LuaVersion::Lua51),
        // 5.2 io.write returns the file handle (chainable); 5.1 doesn't
        // document a return.
        ("io.write", LuaVersion::Lua51),
        // 5.2 io.open rejects modes outside `[rwa]%+?b*`, such as rb+.
        ("io.open", LuaVersion::Lua51),
        // 5.2 string.rep gains the separator argument.
        ("string.rep", LuaVersion::Lua51),
        // 5.2 math.log gains the optional base argument.
        ("math.log", LuaVersion::Lua51),
        // 5.2 pairs/ipairs consult the __pairs/__ipairs metamethods, so
        // any value is a valid subject; 5.1 raw-accesses a table.
        ("pairs", LuaVersion::Lua51),
        ("ipairs", LuaVersion::Lua51),
        // 5.3 math.atan absorbs atan2 via the optional second argument.
        ("math.atan", LuaVersion::Lua52),
        // 5.3 string.dump gains the strip argument.
        ("string.dump", LuaVersion::Lua52),
        // 5.4 userdata carry multiple user values; get/setuservalue gain
        // the index argument.
        ("debug.getuservalue", LuaVersion::Lua53),
        ("debug.setuservalue", LuaVersion::Lua53),
        // collectgarbage changes in every version: 5.2 adds isrunning,
        // generational, and incremental, 5.3 drops the mode switches, 5.4
        // brings them back with tuning arguments, and 5.5 replaces
        // setpause/setstepmul with the param form.
        ("collectgarbage", LuaVersion::Lua51),
        ("collectgarbage", LuaVersion::Lua52),
        ("collectgarbage", LuaVersion::Lua53),
        ("collectgarbage", LuaVersion::Lua54),
        // 5.4 math.randomseed becomes optional and two-seed.
        ("math.randomseed", LuaVersion::Lua53),
        // 5.4 string.gmatch gains the init argument.
        ("string.gmatch", LuaVersion::Lua53),
        ("string:gmatch", LuaVersion::Lua53),
        // 5.4 coroutine.isyieldable gains the coroutine argument.
        ("coroutine.isyieldable", LuaVersion::Lua53),
        // 5.4 drops the __ipairs metamethod 5.3 still runs, so ipairs is
        // pure again.
        ("ipairs", LuaVersion::Lua53),
        // 5.5 coroutine.close defaults to the running coroutine.
        ("coroutine.close", LuaVersion::Lua54),
        // 5.4 utf8 gains the lax flag on codes/codepoint/len.
        ("utf8.codes", LuaVersion::Lua53),
        ("utf8.codepoint", LuaVersion::Lua53),
        ("utf8.len", LuaVersion::Lua53),
        // Derived string receiver mirrors the string.rep delta above.
        ("string:rep", LuaVersion::Lua51),
        // file:lines forwards read formats from 5.2 like io.lines.
        ("file:lines", LuaVersion::Lua51),
        // file:write returns the handle from 5.2 like io.write.
        ("file:write", LuaVersion::Lua51),
    ];

    let mut mismatches = Vec::new();
    for pair in NUMBERED_VERSIONS.windows(2) {
        let &[older, newer] = pair else {
            continue;
        };
        let older_paths = all_entries(standalone_library(older));
        let newer_paths = all_entries(standalone_library(newer));
        for (path, older_entry) in &older_paths {
            let Some(newer_entry) = newer_paths.get(path) else {
                continue;
            };
            let older_fingerprint = fingerprint(older_entry);
            let newer_fingerprint = fingerprint(newer_entry);
            if older_fingerprint != newer_fingerprint
                && !allowlist.contains(&(path.as_str(), older))
            {
                mismatches.push(format!(
                    "{path} ({older:?} -> {newer:?}):\n  {older:?}: {older_fingerprint}\n  {newer:?}: {newer_fingerprint}"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "adjacent numbered-Lua versions drifted without an allowlisted manual delta:\n{}",
        mismatches.join("\n")
    );
}

fn max_placeholder(template: &str) -> usize {
    let mut max = 0;
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%'
            && let Some(digit) = chars.peek().and_then(|next| next.to_digit(10))
        {
            chars.next();
            max = max.max(digit as usize);
        }
    }
    max
}

/// A std definition's format already rules out a function with no
/// signature, params out of order, and an empty constant list.
#[test]
fn overloads_are_distinct_and_constants_unique() {
    let mut violations = Vec::new();
    for (file, library) in all_libraries() {
        for (path, entry) in all_entries(library) {
            let StdlibEntry::Function(function) = entry else {
                continue;
            };
            let mut arities = Vec::new();
            for signature in function.signatures {
                for param in signature.params {
                    if let StdlibArgKind::Constant(values) = param.kind {
                        let mut seen_values = BTreeSet::new();
                        for v in values {
                            if !seen_values.insert(v.value) {
                                violations.push(format!(
                                    "{file} {path}: duplicate constant value '{}'",
                                    v.value
                                ));
                            }
                        }
                    }
                }
                arities.push((signature.min_args, signature.max_args));
            }
            let mut seen_arities = BTreeSet::new();
            for arity in &arities {
                if !seen_arities.insert(*arity) {
                    violations.push(format!(
                        "{file} {path}: two overloads with identical arity range {arity:?}"
                    ));
                }
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn shape_references_resolve_and_templates_fit_arity() {
    let mut violations = Vec::new();
    for (file, library) in all_libraries() {
        for (path, entry) in all_entries(library) {
            let (shape_reference, template_budget) = match entry {
                StdlibEntry::Function(function) => {
                    let budget = function.max_args().unwrap_or(usize::MAX);
                    (function.returns_shape.as_ref(), budget)
                }
                StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                    (value.shape.as_ref(), 0)
                }
                StdlibEntry::Namespace(namespace) => (namespace.shape.as_ref(), 0),
            };
            // A std definition may name a shape of another library, which
            // a built-in one has no use for.
            if let Some(shape) = shape_reference
                && !library.shapes.contains_key(shape)
            {
                violations.push(format!("{file} {path}: dangling shape reference '{shape}'"));
            }
            let mut check_template = |context: &str,
                                      deprecation: &StdlibDeprecation<'_>,
                                      budget: usize| {
                if let Some(replacement) = &deprecation.replacement {
                    let template = replacement.template;
                    let highest = max_placeholder(template);
                    if highest > budget {
                        violations.push(format!(
                            "{file} {path}{context}: template '{template}' references arg {highest}, max arity {budget}"
                        ));
                    }
                }
            };
            if let Some(deprecation) = entry.deprecation() {
                check_template("", deprecation, template_budget);
            }
            if let StdlibEntry::Function(function) = entry {
                for signature in function.signatures {
                    for param in signature.params {
                        if let Some(deprecation) = &param.deprecated {
                            check_template(" (param)", deprecation, template_budget);
                        }
                        if let StdlibArgKind::Constant(values) = param.kind {
                            for v in values {
                                if let Some(deprecation) = &v.deprecated {
                                    check_template(" (constant)", deprecation, template_budget);
                                }
                            }
                        }
                    }
                }
            }
        }
        for (name, shape) in &library.shapes {
            if let Some(base) = shape.extends
                && !library.shapes.contains_key(base)
            {
                violations.push(format!("{file} {name}: extends the missing shape '{base}'"));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// The derived string receiver mirrors each file's string namespace:
/// every subject-first member becomes a method, the two non-receiver
/// members are excluded, and nothing else appears.
#[test]
fn string_shape_mirrors_string_namespace() {
    // string.char builds from byte values and string.dump takes a
    // function; neither has a string subject.
    const NON_RECEIVER_MEMBERS: [&str; 2] = ["char", "dump"];
    for (file, library) in all_libraries() {
        let Some(StdlibEntry::Namespace(namespace)) = library.globals.get("string") else {
            panic!("{file}: string namespace missing");
        };
        let Some(shape) = library.shapes.get("string") else {
            panic!("{file}: derived string shape missing");
        };
        for (name, entry) in &namespace.members {
            let is_function = matches!(entry, StdlibEntry::Function(_));
            let should_be_member = is_function && !NON_RECEIVER_MEMBERS.contains(&name);
            assert_eq!(
                shape.members.contains_key(name),
                should_be_member,
                "{file}: string shape membership wrong for '{name}'"
            );
            if let (Some(StdlibEntry::Function(method)), StdlibEntry::Function(source)) =
                (shape.members.get(name), entry)
            {
                assert!(method.is_method, "{file}: string:{name} not a method");
                assert_eq!(
                    method.primary_signature().params.len(),
                    source.primary_signature().params.len() - 1,
                    "{file}: string:{name} did not drop exactly the subject param"
                );
            }
        }
        for name in shape.members.keys() {
            assert!(
                namespace.members.contains_key(name),
                "{file}: string shape member '{name}' has no namespace source"
            );
        }
    }
}

#[test]
fn environment_surfaces_do_not_leak() {
    // Representative Roblox-only surface; none of it may appear in
    // standalone Luau.
    const ROBLOX_ONLY_GLOBALS: [&str; 24] = [
        "game",
        "workspace",
        "script",
        "plugin",
        "shared",
        "Enum",
        "task",
        "warn",
        "Instance",
        "Vector3",
        "CFrame",
        "Color3",
        "UDim2",
        "DateTime",
        "SharedTable",
        "Content",
        "BrickColor",
        "tick",
        "wait",
        "spawn",
        "delay",
        "elapsedTime",
        "UserSettings",
        "settings",
    ];
    // Both Luau flavors lack the loader/io surface the VM removed.
    const LUAU_ABSENT_GLOBALS: [&str; 6] =
        ["io", "package", "dofile", "loadfile", "load", "module"];
    for name in ROBLOX_ONLY_GLOBALS {
        assert!(!luau().globals.contains_key(name), "{name} leaked into standalone luau");
        assert!(roblox().globals.contains_key(name), "{name} missing from roblox");
    }
    for name in LUAU_ABSENT_GLOBALS {
        assert!(!luau().globals.contains_key(name), "{name} must be absent from standalone luau");
        assert!(!roblox().globals.contains_key(name), "{name} must be absent from roblox luau");
    }
    // Luau-only globals must not leak into numbered Lua, though 5.1 has its
    // own gcinfo.
    for version in NUMBERED_VERSIONS {
        for name in ["typeof", "buffer", "vector", "gcinfo"] {
            let should_be_present = name == "gcinfo" && version == LuaVersion::Lua51;
            assert_eq!(
                standalone_library(version).globals.contains_key(name),
                should_be_present,
                "{name} presence wrong in {version:?}"
            );
        }
    }
}
