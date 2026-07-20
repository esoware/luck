//! Drift guards for the split stdlib data files.
//!
//! The seven per-environment files are deliberately independent - no
//! inheritance or layering - so nothing structural stops two copies of
//! a shared entry from drifting apart. These tests are the enforcement:
//! every surface shared between files must agree exactly unless the
//! divergence is listed here with its justification. The original
//! stdlib audit found exactly the class of bugs this suite mechanizes
//! (signatures diverging across versions with no manual basis, markers
//! present on some siblings and missing on others).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use luck_semantic::stdlib_model::{StdlibArgKind, StdlibEntry, StdlibLibrary, library_for};
use luck_token::{LuaVersion, StdlibEnvironment};

const NUMBERED: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn numbered(version: LuaVersion) -> &'static StdlibLibrary {
    library_for(version, StdlibEnvironment::Standalone)
}

fn luau() -> &'static StdlibLibrary {
    library_for(LuaVersion::Luau, StdlibEnvironment::Standalone)
}

fn roblox() -> &'static StdlibLibrary {
    library_for(LuaVersion::Luau, StdlibEnvironment::Roblox)
}

/// Flatten a library's dotted global surface: `("math.floor", entry)`.
fn global_paths(lib: &'static StdlibLibrary) -> BTreeMap<String, &'static StdlibEntry> {
    let mut out = BTreeMap::new();
    for (name, entry) in &lib.globals {
        collect(name, entry, &mut out);
    }
    out
}

/// Flatten a library's shape surface: `("file:read", entry)`.
fn shape_paths(lib: &'static StdlibLibrary) -> BTreeMap<String, &'static StdlibEntry> {
    let mut out = BTreeMap::new();
    for (shape_name, shape) in &lib.shapes {
        for (member, entry) in &shape.members {
            collect(&format!("{shape_name}:{member}"), entry, &mut out);
        }
    }
    out
}

fn collect(
    path: &str,
    entry: &'static StdlibEntry,
    out: &mut BTreeMap<String, &'static StdlibEntry>,
) {
    out.insert(path.to_string(), entry);
    if let StdlibEntry::Namespace(namespace) = entry {
        for (name, child) in &namespace.members {
            collect(&format!("{path}.{name}"), child, out);
        }
    }
}

/// Canonical signature-and-metadata rendering, deliberately excluding
/// deprecation (compared separately: it legitimately differs across
/// versions as APIs age).
fn fingerprint(entry: &StdlibEntry) -> String {
    match entry {
        StdlibEntry::Function(func) => {
            let mut out = String::from("fn");
            if func.is_pure {
                out.push_str(" pure");
            }
            if func.must_use {
                out.push_str(" must_use");
            }
            if func.is_method {
                out.push_str(" method");
            }
            if !func.read_only {
                out.push_str(" writable");
            }
            if let Some(shape) = &func.returns_shape {
                let _ = write!(out, " -> {shape}");
            }
            for sig in &func.signatures {
                let _ = write!(
                    out,
                    " [{}..{}](",
                    sig.min_args,
                    sig.max_args.map_or("*".to_string(), |n| n.to_string())
                );
                for param in &sig.params {
                    let kind = match &param.kind {
                        StdlibArgKind::Any => "any".to_string(),
                        StdlibArgKind::Bool => "bool".to_string(),
                        StdlibArgKind::Number => "number".to_string(),
                        StdlibArgKind::String => "string".to_string(),
                        StdlibArgKind::Function => "function".to_string(),
                        StdlibArgKind::Table => "table".to_string(),
                        StdlibArgKind::Nil => "nil".to_string(),
                        StdlibArgKind::Vararg => "...".to_string(),
                        StdlibArgKind::Display(label) => format!("<{label}>"),
                        StdlibArgKind::Constant(values) => {
                            let rendered: Vec<&str> =
                                values.iter().map(|v| v.value.as_str()).collect();
                            format!("constant{{{}}}", rendered.join(","))
                        }
                    };
                    let _ = write!(
                        out,
                        "{}{}{},",
                        kind,
                        if param.required { "" } else { "?" },
                        if param.deprecated.is_some() { "!" } else { "" }
                    );
                }
                out.push(')');
            }
            out
        }
        StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
            let kind = if matches!(entry, StdlibEntry::Constant(_)) {
                "const"
            } else {
                "prop"
            };
            format!(
                "{kind}{}{}",
                if value.read_only { "" } else { " writable" },
                value
                    .shape
                    .as_ref()
                    .map_or(String::new(), |shape| format!(" : {shape}"))
            )
        }
        StdlibEntry::Namespace(namespace) => format!(
            "namespace{}",
            namespace
                .shape
                .as_ref()
                .map_or(String::new(), |shape| format!(" : {shape}"))
        ),
    }
}

fn deprecation_fingerprint(entry: &StdlibEntry) -> String {
    match entry.deprecation() {
        None => "live".to_string(),
        Some(deprecation) => format!(
            "deprecated({}|{})",
            deprecation.message,
            deprecation.replace_template.as_deref().unwrap_or("-")
        ),
    }
}

// ---- luau.toml vs luau_roblox.toml ----

/// Shared Luau surface: identical in both environments except the
/// explicit allowlists below.
#[test]
fn luau_and_roblox_shared_surface_agrees() {
    // Signature divergence: Roblox vectors are 3-wide, standalone Luau
    // additionally ships the LUA_VECTOR_SIZE=4 build's 4-arg form.
    const SIGNATURE_ALLOWLIST: [&str; 1] = ["vector.create"];
    // Deprecation divergence: Roblox deprecates these while they stay
    // fully supported in standalone Luau; the split files express what
    // tier scoping used to.
    const DEPRECATION_ALLOWLIST: [&str; 3] = ["collectgarbage", "getfenv", "setfenv"];

    let standalone_globals = global_paths(luau());
    let roblox_globals = global_paths(roblox());
    let mut mismatches = Vec::new();
    for (path, standalone_entry) in &standalone_globals {
        let Some(roblox_entry) = roblox_globals.get(path) else {
            continue;
        };
        let standalone_print = fingerprint(standalone_entry);
        let roblox_print = fingerprint(roblox_entry);
        if standalone_print != roblox_print && !SIGNATURE_ALLOWLIST.contains(&path.as_str()) {
            mismatches.push(format!(
                "{path}:\n  standalone: {standalone_print}\n  roblox:     {roblox_print}"
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

    // The only shape both environments define is the derived string
    // receiver; it mirrors the string namespaces compared above, but a
    // hand-authored override landing in one file would surface here.
    let standalone_shapes = shape_paths(luau());
    let roblox_shapes = shape_paths(roblox());
    for (path, standalone_entry) in &standalone_shapes {
        let Some(roblox_entry) = roblox_shapes.get(path) else {
            continue;
        };
        let standalone_print = fingerprint(standalone_entry);
        let roblox_print = fingerprint(roblox_entry);
        if standalone_print != roblox_print {
            mismatches.push(format!(
                "shape {path}:\n  standalone: {standalone_print}\n  roblox:     {roblox_print}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "luau.toml and luau_roblox.toml drifted on shared entries:\n{}",
        mismatches.join("\n")
    );
}

// ---- numbered Lua adjacency ----

/// Entries shared by consecutive numbered versions must be identical
/// unless the manuals changed them. Every allowlisted pair cites the
/// actual manual delta; deprecation metadata is deliberately not
/// compared here (APIs aging into deprecation across versions is the
/// norm, and marker policy has its own tests).
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
        // 5.2 io.write returns the file handle (chainable); 5.1 does not
        // document a return.
        ("io.write", LuaVersion::Lua51),
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
        // GC interface churn: option sets and tuning arity change in
        // every version (isrunning/generational/incremental in 5.2,
        // set reduced in 5.3, overloaded tuning forms in 5.4, the
        // param form replacing setpause/setstepmul in 5.5).
        ("collectgarbage", LuaVersion::Lua51),
        ("collectgarbage", LuaVersion::Lua52),
        ("collectgarbage", LuaVersion::Lua53),
        ("collectgarbage", LuaVersion::Lua54),
        // 5.4 math.randomseed becomes optional and two-seed.
        ("math.randomseed", LuaVersion::Lua53),
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
    for pair in NUMBERED.windows(2) {
        let (older, newer) = (pair[0], pair[1]);
        let older_paths: BTreeMap<String, &StdlibEntry> = global_paths(numbered(older))
            .into_iter()
            .chain(shape_paths(numbered(older)))
            .collect();
        let newer_paths: BTreeMap<String, &StdlibEntry> = global_paths(numbered(newer))
            .into_iter()
            .chain(shape_paths(numbered(newer)))
            .collect();
        for (path, older_entry) in &older_paths {
            let Some(newer_entry) = newer_paths.get(path) else {
                continue;
            };
            let older_print = fingerprint(older_entry);
            let newer_print = fingerprint(newer_entry);
            if older_print != newer_print && !allowlist.contains(&(path.as_str(), older)) {
                mismatches.push(format!(
                    "{path} ({older:?} -> {newer:?}):\n  {older:?}: {older_print}\n  {newer:?}: {newer_print}"
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

// ---- structural invariants ----

fn all_libraries() -> Vec<(&'static str, &'static StdlibLibrary)> {
    vec![
        ("lua51", numbered(LuaVersion::Lua51)),
        ("lua52", numbered(LuaVersion::Lua52)),
        ("lua53", numbered(LuaVersion::Lua53)),
        ("lua54", numbered(LuaVersion::Lua54)),
        ("lua55", numbered(LuaVersion::Lua55)),
        ("luau", luau()),
        ("luau_roblox", roblox()),
    ]
}

fn all_entries(lib: &'static StdlibLibrary) -> BTreeMap<String, &'static StdlibEntry> {
    let mut out = global_paths(lib);
    out.extend(shape_paths(lib));
    out
}

/// Highest `%n` placeholder in a replace template.
fn max_placeholder(template: &str) -> usize {
    let mut max = 0;
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%'
            && let Some(digit) = chars.peek().and_then(|c| c.to_digit(10))
        {
            chars.next();
            max = max.max(digit as usize);
        }
    }
    max
}

#[test]
fn signature_shapes_are_internally_consistent() {
    let mut violations = Vec::new();
    for (file, lib) in all_libraries() {
        for (path, entry) in all_entries(lib) {
            let StdlibEntry::Function(func) = entry else {
                continue;
            };
            if func.signatures.is_empty() {
                violations.push(format!("{file} {path}: function with no signatures"));
                continue;
            }
            let mut arities = Vec::new();
            for sig in &func.signatures {
                let has_vararg = sig
                    .params
                    .last()
                    .is_some_and(|param| matches!(param.kind, StdlibArgKind::Vararg));
                if sig
                    .params
                    .iter()
                    .rev()
                    .skip(1)
                    .any(|param| matches!(param.kind, StdlibArgKind::Vararg))
                {
                    violations.push(format!("{file} {path}: vararg param not in last position"));
                }
                match sig.max_args {
                    None => {
                        if !has_vararg {
                            violations.push(format!(
                                "{file} {path}: unbounded max_args without trailing vararg"
                            ));
                        }
                    }
                    Some(max) => {
                        if sig.min_args > max {
                            violations.push(format!("{file} {path}: min_args > max_args"));
                        }
                        if !has_vararg && max != sig.params.len() {
                            violations.push(format!(
                                "{file} {path}: max_args {max} != {} declared params",
                                sig.params.len()
                            ));
                        }
                    }
                }
                let required = sig
                    .params
                    .iter()
                    .filter(|param| param.required && !matches!(param.kind, StdlibArgKind::Vararg))
                    .count();
                if required != sig.min_args {
                    violations.push(format!(
                        "{file} {path}: min_args {} != {required} required params",
                        sig.min_args
                    ));
                }
                for param in &sig.params {
                    if let StdlibArgKind::Constant(values) = &param.kind {
                        if values.is_empty() {
                            violations.push(format!("{file} {path}: empty constant set"));
                        }
                        let mut seen = std::collections::BTreeSet::new();
                        for value in values {
                            if !seen.insert(value.value.as_str()) {
                                violations.push(format!(
                                    "{file} {path}: duplicate constant value '{}'",
                                    value.value
                                ));
                            }
                        }
                    }
                }
                arities.push((sig.min_args, sig.max_args));
            }
            let mut seen = std::collections::BTreeSet::new();
            for arity in &arities {
                if !seen.insert(*arity) {
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
    for (file, lib) in all_libraries() {
        for (path, entry) in all_entries(lib) {
            let (shape_ref, template_budget) = match entry {
                StdlibEntry::Function(func) => {
                    let budget = func.max_args().unwrap_or(usize::MAX);
                    (func.returns_shape.as_ref(), budget)
                }
                StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                    (value.shape.as_ref(), 0)
                }
                StdlibEntry::Namespace(namespace) => (namespace.shape.as_ref(), 0),
            };
            if let Some(shape) = shape_ref
                && !lib.shapes.contains_key(shape.as_str())
            {
                violations.push(format!("{file} {path}: dangling shape reference '{shape}'"));
            }
            let mut check_template =
                |context: &str,
                 deprecation: &luck_semantic::stdlib_model::StdlibDeprecation,
                 budget: usize| {
                    if let Some(template) = &deprecation.replace_template {
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
            if let StdlibEntry::Function(func) = entry {
                for sig in &func.signatures {
                    for param in &sig.params {
                        if let Some(deprecation) = &param.deprecated {
                            check_template(" (param)", deprecation, template_budget);
                        }
                        if let StdlibArgKind::Constant(values) = &param.kind {
                            for value in values {
                                if let Some(deprecation) = &value.deprecated {
                                    check_template(" (constant)", deprecation, template_budget);
                                }
                            }
                        }
                    }
                }
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
    const EXCLUDED: [&str; 2] = ["char", "dump"];
    for (file, lib) in all_libraries() {
        let Some(StdlibEntry::Namespace(namespace)) = lib.globals.get("string") else {
            panic!("{file}: string namespace missing");
        };
        let Some(shape) = lib.shapes.get("string") else {
            panic!("{file}: derived string shape missing");
        };
        for (name, entry) in &namespace.members {
            let is_function = matches!(entry, StdlibEntry::Function(_));
            let expected = is_function && !EXCLUDED.contains(&name.as_str());
            assert_eq!(
                shape.members.contains_key(name),
                expected,
                "{file}: string shape membership wrong for '{name}'"
            );
            if let Some(StdlibEntry::Function(method)) = shape.members.get(name) {
                assert!(method.is_method, "{file}: string:{name} not a method");
                let StdlibEntry::Function(source) = entry else {
                    unreachable!()
                };
                assert_eq!(
                    method.primary_signature().params.len(),
                    source.primary_signature().params.len().saturating_sub(1),
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

// ---- environment invariants ----

#[test]
fn environment_surfaces_do_not_leak() {
    // Representative Roblox-only surface; none of it may appear in
    // standalone Luau.
    const ROBLOX_ONLY: [&str; 24] = [
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
    for name in ROBLOX_ONLY {
        assert!(
            !luau().globals.contains_key(name),
            "{name} leaked into standalone luau"
        );
        assert!(
            roblox().globals.contains_key(name),
            "{name} missing from roblox"
        );
    }
    // Both Luau flavors lack the loader/io surface the VM removed.
    const LUAU_ABSENT: [&str; 6] = ["io", "package", "dofile", "loadfile", "load", "module"];
    for name in LUAU_ABSENT {
        assert!(
            !luau().globals.contains_key(name),
            "{name} must be absent from standalone luau"
        );
        assert!(
            !roblox().globals.contains_key(name),
            "{name} must be absent from roblox luau"
        );
    }
    // Luau-only libraries must not leak into numbered Lua.
    for version in NUMBERED {
        for name in ["typeof", "buffer", "vector", "gcinfo"] {
            let allowed = name == "gcinfo" && version == LuaVersion::Lua51;
            assert_eq!(
                numbered(version).globals.contains_key(name),
                allowed,
                "{name} presence wrong in {version:?}"
            );
        }
    }
}

// ---- audit spot-check replay ----

/// The original audit's headline findings, pinned forever.
#[test]
fn audit_spot_checks_hold() {
    // Phantoms stay gone.
    for lib in [luau(), roblox()] {
        for name in ["isnan", "isinf", "isfinite"] {
            assert!(
                lib.lookup_str(&["math", name]).is_none(),
                "phantom math.{name} returned"
            );
        }
        assert!(lib.globals.contains_key("loadstring"));
        let Some(StdlibEntry::Function(rep)) = lib.lookup_str(&["string", "rep"]) else {
            panic!("string.rep missing");
        };
        assert_eq!(rep.max_args(), Some(2), "Luau string.rep grew a separator");
    }
    for (_, lib) in all_libraries() {
        assert!(
            !lib.globals.contains_key("bit"),
            "phantom bit namespace returned"
        );
    }
    // 5.2 debug quartet present.
    for name in ["getuservalue", "setuservalue", "upvalueid", "upvaluejoin"] {
        assert!(
            numbered(LuaVersion::Lua52)
                .lookup_str(&["debug", name])
                .is_some(),
            "5.2 debug.{name} missing"
        );
    }
    // 5.5 table.create takes a hash-size hint, not Luau's fill value.
    let Some(StdlibEntry::Function(create)) =
        numbered(LuaVersion::Lua55).lookup_str(&["table", "create"])
    else {
        panic!("5.5 table.create missing");
    };
    assert!(
        matches!(
            create.primary_signature().params[1].kind,
            StdlibArgKind::Number
        ),
        "5.5 table.create second param must be nrec: number"
    );
    // GetService carries the service set with Players in it.
    let Some(StdlibEntry::Function(get_service)) = roblox().shape_member("DataModel", "GetService")
    else {
        panic!("game:GetService missing");
    };
    let StdlibArgKind::Constant(services) = &get_service.primary_signature().params[0].kind else {
        panic!("GetService param not a constant set");
    };
    assert!(
        services.len() > 300,
        "service set shrank: {}",
        services.len()
    );
    assert!(services.iter().any(|s| s.value == "Players" && s.is_common));
    // Enum tree populated and resolvable.
    assert!(
        roblox()
            .lookup_str(&["Enum", "Material", "Grass"])
            .is_some()
    );
    assert!(!luau().globals.contains_key("Enum"));
    // Roblox-only deprecations respect the split.
    for name in ["collectgarbage", "getfenv", "setfenv"] {
        assert!(
            roblox().globals[name].deprecation().is_some(),
            "{name} must be deprecated on roblox"
        );
        assert!(
            luau().globals[name].deprecation().is_none(),
            "{name} must be live standalone"
        );
    }
}
