use std::ptr;

use luck_stdlib::{
    SecurityLevel, Stdlib, StdlibArgKind, StdlibEntry, StdlibFunction, StdlibLibrary, library_for,
    luau_library, roblox_library,
};

#[test]
#[cfg(target_pointer_width = "64")]
fn stdlib_entry_is_two_words() {
    assert_eq!(size_of::<StdlibEntry<'_>>(), 16);
}

fn layered(text: &str) -> Stdlib {
    Stdlib::luau().layer(text).expect("the definitions parse")
}

fn function<'lib>(library: &'lib StdlibLibrary<'lib>, path: &[&str]) -> &'lib StdlibFunction<'lib> {
    let Some(StdlibEntry::Function(function)) = library.lookup(path) else {
        panic!("{path:?} should be a function");
    };
    function
}

fn method<'lib>(
    library: &'lib StdlibLibrary<'lib>,
    shape: &str,
    name: &str,
) -> &'lib StdlibFunction<'lib> {
    let Some(StdlibEntry::Function(function)) = library.shape_member(shape, name) else {
        panic!("{shape} should have the method {name}");
    };
    function
}

/// Each signature's params, as `string`, the constants it allows, or `_` for
/// any other type, with `?` after an optional one, or as `...`.
fn params(function: &StdlibFunction<'_>) -> Vec<Vec<String>> {
    function
        .signatures
        .iter()
        .map(|signature| {
            signature
                .params
                .iter()
                .map(|param| {
                    let kind = match param.kind {
                        StdlibArgKind::String => "string".to_owned(),
                        StdlibArgKind::Constant(values) => format!("{values:?}"),
                        StdlibArgKind::Vararg => return "...".to_owned(),
                        StdlibArgKind::Other => "_".to_owned(),
                    };
                    if param.is_required { kind } else { format!("{kind}?") }
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_declared_function_takes_its_named_params() {
    let stdlib = layered(
        "
declare function connect(host: string, port: number?, options: { secure: boolean }?): Socket

declare extern type Socket with
    function send(self, data: string, ...: any): ()
end
",
    );
    let library = stdlib.library();
    let connect = function(library, &["connect"]);
    assert_eq!(params(connect), [["string", "_?", "_?"]]);
    assert_eq!((connect.min_args(), connect.max_args()), (1, Some(3)));
    assert_eq!(connect.returns_shape, Some("Socket"));
    let send = method(library, "Socket", "send");
    assert!(send.is_method);
    assert_eq!(params(send), [["string", "..."]]);
    assert!(library.lookup(["print"]).is_some(), "the built-in library stays under it");
}

#[test]
fn a_table_type_is_a_namespace_of_its_fields_and_a_function_type_a_function() {
    let stdlib = layered(
        "
declare app: {
    name: string,
    net: { listen: (port: number) -> () },
}
declare greet: (name: string) -> string
",
    );
    let library = stdlib.library();
    assert!(matches!(library.lookup(["app"]), Some(StdlibEntry::Namespace(_))));
    assert!(matches!(library.lookup(["app", "name"]), Some(StdlibEntry::Value(_))));
    assert!(matches!(library.lookup(["app", "net"]), Some(StdlibEntry::Namespace(_))));
    assert_eq!(params(function(library, &["app", "net", "listen"])), [["_"]]);
    assert_eq!(params(function(library, &["greet"])), [["string"]]);
}

#[test]
fn an_extern_type_is_a_value_of_its_shape_with_its_supertype_s_members() {
    let stdlib = layered(
        "
declare extern type Instance with
    Name: string
    Parent: Instance?
    function FindFirstChild(self, name: string): Instance?
end

declare extern type DataModel extends Instance with
    function GetService(self, name: string): Instance
end

declare game: DataModel
",
    );
    let library = stdlib.library();
    assert_eq!(library.lookup(["game"]).and_then(StdlibEntry::shape), Some("DataModel"));
    assert_eq!(method(library, "DataModel", "FindFirstChild").returns_shape, Some("Instance"));
    assert_eq!(library.lookup(["game", "Parent"]).and_then(StdlibEntry::shape), Some("Instance"));
    assert_eq!(library.lookup(["game", "Name"]).and_then(StdlibEntry::shape), Some("string"));
    assert!(library.lookup(["game", "Parent", "GetService"]).is_none());
}

#[test]
fn a_param_is_optional_when_its_type_takes_nil() {
    let stdlib =
        layered("declare function f(a: number?, b: nil | string, c: any, d: unknown, e: boolean)");
    let f = function(stdlib.library(), &["f"]);
    assert_eq!(params(f), [["_?", "string?", "_?", "_?", "_"]]);
    assert_eq!(
        f.min_args(),
        5,
        "a call leaves out only the optional params after the last required one"
    );
}

#[test]
fn a_trailing_variadic_or_generic_pack_is_a_vararg() {
    let stdlib = layered(
        "
declare function log(level: number, ...: string)
declare run: <A...>(callback: (A...) -> (), A...) -> ()
declare sum: (number, ...number) -> number
",
    );
    let library = stdlib.library();
    for (path, expected) in [("log", ["_", "..."]), ("run", ["_", "..."]), ("sum", ["_", "..."])] {
        let found = function(library, &[path]);
        assert_eq!(params(found), [expected], "{path}");
        assert_eq!(found.max_args(), None, "{path}");
    }
}

#[test]
fn a_string_singleton_union_is_the_constants_it_allows() {
    let stdlib = layered(
        r#"
type Mode = "r" | "w"
declare function open(path: string, mode: Mode?, flag: "a" | "b" | "a")
declare function seek(whence: "set" | "cur" | number)
"#,
    );
    let library = stdlib.library();
    let open = function(library, &["open"]);
    assert_eq!(params(open), [["string", r#"["r", "w"]?"#, r#"["a", "b"]"#]]);
    assert_eq!(
        open.allowed_constants(1, 3).map(Iterator::collect::<Vec<_>>),
        Some(vec![&["r", "w"][..]])
    );
    assert_eq!(params(function(library, &["seek"])), [["_"]]);
}

#[test]
fn a_generic_takes_anything_and_names_no_shape() {
    let stdlib = layered(
        "
declare extern type T with
end
declare function id<T>(value: T): T
declare function make(): T
",
    );
    let library = stdlib.library();
    let id = function(library, &["id"]);
    assert_eq!(params(id), [["_"]]);
    assert_eq!(id.returns_shape, None);
    assert_eq!(function(library, &["make"]).returns_shape, Some("T"));
}

#[test]
fn an_intersection_of_function_types_or_a_repeated_method_is_overloads() {
    let stdlib = layered(
        "
declare pick: ((n: number) -> number) & ((a: string, b: string) -> string)

declare extern type Picker with
    function Pick(self, n: number): number
    function Pick(self, a: string, b: string): string
end
",
    );
    let library = stdlib.library();
    let expected = [vec!["_"], vec!["string", "string"]];
    assert_eq!(params(function(library, &["pick"])), expected);
    assert_eq!(params(method(library, "Picker", "Pick")), expected);
    assert!(!function(library, &["pick"]).accepts_arg_count(0));
}

#[test]
fn a_deprecation_comes_from_its_attribute() {
    let stdlib = layered(
        r#"
@deprecated declare function old()
@[deprecated {use = "new.call", reason = "old is slow"}] declare function older()
declare lib: { gone: @[deprecated {reason = "use `#t` instead"}] (t: {}) -> number }

declare extern type Thing with
    @[deprecated {use = "Destroy"}] function remove(self): ()
end
"#,
    );
    let library = stdlib.library();
    let deprecation = |path: &[&str]| {
        let deprecation = function(library, path).deprecated.expect("it's deprecated");
        (deprecation.r#use, deprecation.reason)
    };
    assert_eq!(deprecation(&["old"]), (None, None));
    assert_eq!(deprecation(&["older"]), (Some("new.call"), Some("old is slow")));
    assert_eq!(deprecation(&["lib", "gone"]), (None, Some("use `#t` instead")));
    let remove = method(library, "Thing", "remove").deprecated.expect("it's deprecated");
    assert_eq!((remove.r#use, remove.reason), (Some("Destroy"), None));
    assert!(function(library, &["print"]).deprecated.is_none());
}

#[test]
fn a_replacement_takes_the_calls_its_declared_types_take() {
    let stdlib = layered(
        r#"
declare app: {
    open: (path: string, mode: ("r" | "w")?) -> boolean,
    run: <A..., R...>(callback: thread | (A...) -> R..., A...) -> thread,
}
@[deprecated {use = "app.open"}] declare function load(path: string): boolean
@[deprecated {use = "app.open"}] declare function fetch(path: number): boolean
@[deprecated {use = "app.run"}] declare function call(callback: () -> ())
@[deprecated {use = "print"}] declare function log(...: any)

declare extern type Door with
    @[deprecated {use = "Open"}] function open(self, force: boolean?): ()
    function Open(self, force: boolean?, speed: number?): ()
end
"#,
    );
    let library = stdlib.library();
    let replacement = |path: &[&str]| function(library, path).deprecated?.replacement;

    let load = replacement(&["load"]).expect("`app.open` is declared");
    assert!(!load.is_method);
    assert!(load.takes(1, true), "`app.open` takes a path and returns a boolean");
    assert!(!load.takes(0, false) && !load.takes(2, false), "`load` takes one argument");

    let fetch = replacement(&["fetch"]).expect("`app.open` is declared");
    assert!(!fetch.takes(1, false), "`app.open` takes no number");

    let call = replacement(&["call"]).expect("`app.run` is declared");
    assert!(call.takes(1, false), "`app.run` takes any function");
    assert!(!call.takes(1, true), "`app.run` returns a thread, and `call` nothing");

    assert_eq!(replacement(&["log"]), None, "the file doesn't declare `print`");

    let open = method(library, "Door", "open").deprecated.and_then(|it| it.replacement);
    let open = open.expect("`Open` is declared");
    assert!(open.is_method);
    assert!(open.takes(0, true) && open.takes(1, true));
}

#[test]
fn a_declared_global_is_read_only_and_a_field_only_when_declared_read() {
    let stdlib = layered(
        "
declare config: { read version: string, name: string, run: () -> () }
declare function start()
declare count: number

declare extern type Part with
    read Id: number
    Size: number
    function Clone(self): Part
end
",
    );
    let library = stdlib.library();
    let is_read_only = |path: &[&str]| library.lookup(path).is_some_and(StdlibEntry::is_read_only);
    assert!(is_read_only(&["config"]));
    assert!(is_read_only(&["start"]));
    assert!(is_read_only(&["count"]));
    assert!(is_read_only(&["config", "version"]));
    assert!(!is_read_only(&["config", "name"]));
    assert!(!is_read_only(&["config", "run"]));
    let member =
        |name: &str| library.shape_member("Part", name).is_some_and(StdlibEntry::is_read_only);
    assert!(member("Id"));
    assert!(!member("Size"));
    assert!(member("Clone"));
}

#[test]
fn a_string_function_that_takes_a_string_first_is_a_method_of_strings() {
    let stdlib = layered(
        "declare string: typeof(string) & { trim: (s: string) -> string, fromcodes: (...number) -> string }",
    );
    let library = stdlib.library();
    let trim = method(library, "string", "trim");
    assert!(trim.is_method);
    assert_eq!(trim.max_args(), Some(0));
    assert_eq!(trim.returns_shape, Some("string"));
    assert!(library.shape_member("string", "fromcodes").is_none());
    assert!(library.shape_member("string", "upper").is_some(), "`typeof(string)` keeps Luau's");
    assert!(library.lookup(["string", "format"]).is_some());
}

#[test]
fn a_later_declaration_replaces_an_earlier_one() {
    let stdlib = Stdlib::luau();
    let first = stdlib
        .layer(
            "
declare function describe(name: string, body: () -> ())
declare extern type Point with
    X: number
end
",
        )
        .expect("the first parses");
    let second = first
        .layer(
            "
declare function describe(name: string, body: (() -> ())?)
declare extern type Point with
    Y: number
end
",
        )
        .expect("the second parses");
    let arity = |layer: &Stdlib| function(layer.library(), &["describe"]).min_args();
    assert_eq!(arity(&first), 2, "a layer leaves the library it's over as it was");
    assert_eq!(arity(&second), 1);
    assert!(stdlib.library().lookup(["describe"]).is_none());
    assert!(second.library().shape_member("Point", "Y").is_some());
    assert!(second.library().shape_member("Point", "X").is_none(), "a redeclaration isn't merged");
}

#[test]
fn typeof_reads_the_global_declared_before_it() {
    let stdlib = layered("declare table: typeof(table) & { shuffle: (t: {any}) -> () }");
    let library = stdlib.library();
    assert!(library.lookup(["table", "shuffle"]).is_some());
    assert!(library.lookup(["table", "insert"]).is_some());
}

#[test]
fn a_definition_that_doesnt_parse_reports_where() {
    let source = "declare function f(: number)";
    let diags = Stdlib::luau().layer(source).expect_err("the definition doesn't parse");
    assert_eq!(diags[0].code, "E008", "{diags:?}");
    assert!(diags[0].span.is_some_and(|span| span.end as usize <= source.len()), "{diags:?}");
}

fn all_libraries() -> Vec<&'static StdlibLibrary<'static>> {
    let roblox = SecurityLevel::ALL.map(roblox_library);
    [luau_library()].into_iter().chain(roblox).collect()
}

fn label(library: &StdlibLibrary<'_>) -> &'static str {
    if library.has_roblox { "Roblox" } else { "Luau" }
}

#[test]
fn a_process_builds_each_built_in_library_once_and_roblox_s_at_each_level() {
    assert!(!luau_library().has_roblox);
    let roblox = roblox_library(SecurityLevel::None);
    assert!(roblox.has_roblox);
    assert!(ptr::eq(roblox, roblox_library(SecurityLevel::None)));
    assert!(ptr::eq(Stdlib::luau().library(), luau_library()));
    assert!(ptr::eq(Stdlib::luau().with_roblox(SecurityLevel::None).library(), roblox));
    assert!(ptr::eq(library_for(true), roblox) && ptr::eq(library_for(false), luau_library()));
    let at_plugin = roblox_library(SecurityLevel::PluginSecurity);
    assert!(roblox.shape_member("Instance", "GetDebugId").is_none());
    assert!(at_plugin.shape_member("Instance", "GetDebugId").is_some());
}

#[test]
fn roblox_s_api_layers_over_a_project_s_definitions_too() {
    let stdlib = layered("declare app: { start: () -> () }").with_roblox(SecurityLevel::None);
    assert!(stdlib.library().has_roblox);
    assert!(stdlib.library().lookup(["app", "start"]).is_some());
    assert!(stdlib.library().lookup(["game", "GetService"]).is_some());
    let over = stdlib.layer("declare function greet()").expect("it parses");
    assert!(over.library().has_roblox, "a layer over Roblox's API keeps it");
}

/// Luau binds these in its C++, so they come from its library reference.
#[test]
fn every_library_has_what_luau_binds_past_its_definitions() {
    for library in all_libraries() {
        for path in [
            &["print"][..],
            &["pairs"],
            &["next"],
            &["setmetatable"],
            &["getmetatable"],
            &["string", "format"],
            &["table", "clone"],
            &["table", "freeze"],
        ] {
            assert!(library.lookup(path).is_some(), "{}: {path:?}", label(library));
        }
    }
}

#[test]
fn table_getn_is_deprecated_with_the_reference_s_advice() {
    for library in all_libraries() {
        let deprecation = function(library, &["table", "getn"]).deprecated.expect("deprecated");
        assert_eq!((deprecation.r#use, deprecation.reason), (None, Some("use `#t` instead")));
    }
}

/// Every function of `string` that takes a string first is a method, and
/// `string.char`, which builds a string from bytes, isn't.
#[test]
fn the_string_shape_mirrors_the_string_namespace() {
    for library in all_libraries() {
        let Some(StdlibEntry::Namespace(namespace)) = library.globals.get("string") else {
            panic!("{}: the string namespace is missing", label(library));
        };
        let shape = library.shapes.get("string").expect("the string shape");
        for (name, entry) in &namespace.members {
            let StdlibEntry::Function(source) = entry else { continue };
            assert_eq!(shape.members.contains_key(name), name != "char", "{name}");
            if let Some(StdlibEntry::Function(method)) = shape.members.get(name) {
                assert!(method.is_method, "{name}");
                assert_eq!(
                    method.signatures[0].params.len() + 1,
                    source.signatures[0].params.len(),
                    "{name}"
                );
            }
        }
        assert!(shape.members.iter().all(|(name, _)| namespace.members.contains_key(name)));
    }
}

#[test]
fn roblox_globals_are_only_in_roblox_s_api() {
    let roblox = roblox_library(SecurityLevel::None);
    for name in
        ["game", "workspace", "script", "Enum", "task", "warn", "Instance", "Vector3", "wait"]
    {
        assert!(luau_library().lookup([name]).is_none(), "{name}");
        assert!(roblox.lookup([name]).is_some(), "{name}");
    }
}

#[test]
fn roblox_values_resolve_through_their_shapes() {
    let roblox = roblox_library(SecurityLevel::None);
    let get_service = function(roblox, &["game", "GetService"]);
    assert!(get_service.is_method);
    assert_eq!(get_service.returns_shape, Some("Instance"));
    assert!(roblox.lookup(["workspace", "Raycast"]).is_some());
    assert!(roblox.lookup(["script", "Parent", "FindFirstChild"]).is_some());
    let grass = roblox.lookup(["Enum", "Material", "Grass"]).expect("Enum.Material.Grass");
    assert_eq!(grass.shape(), Some("EnumMaterial"));
    assert!(roblox.lookup(["Enum", "Material", "Grass", "Name"]).is_some(), "an EnumItem");
    assert!(roblox.lookup(["Enum", "Material", "GetEnumItems"]).is_some(), "an Enum");
}

#[test]
fn roblox_deprecations_say_what_to_use() {
    let roblox = roblox_library(SecurityLevel::None);
    let wait = function(roblox, &["wait"]).deprecated.expect("wait is deprecated");
    assert_eq!(wait.r#use, Some("task.wait"));
    assert!(wait.reason.is_some_and(|reason| reason.contains("task.wait")), "{wait:?}");
    let children = method(roblox, "Instance", "children").deprecated.expect("deprecated");
    assert_eq!(children.r#use, Some("GetChildren"));
}

/// Every shape a built-in library's values, results, and shapes name is one
/// it declares.
#[test]
fn every_shape_a_built_in_library_names_is_declared() {
    fn check(
        library: &StdlibLibrary<'_>,
        path: &str,
        entry: &StdlibEntry<'_>,
        missing: &mut Vec<String>,
    ) {
        let named = match entry {
            StdlibEntry::Function(function) => function.returns_shape,
            StdlibEntry::Value(_) | StdlibEntry::Namespace(_) => entry.shape(),
        };
        if let Some(shape) = named
            && !library.shapes.contains_key(shape)
        {
            missing.push(format!("{}: {path} names {shape}", label(library)));
        }
        if let StdlibEntry::Namespace(namespace) = entry {
            for (name, member) in &namespace.members {
                check(library, &format!("{path}.{name}"), member, missing);
            }
        }
    }
    let mut missing = Vec::new();
    for library in all_libraries() {
        for (name, entry) in &library.globals {
            check(library, name, entry, &mut missing);
        }
        for (shape_name, shape) in &library.shapes {
            for (name, member) in &shape.members {
                check(library, &format!("{shape_name}:{name}"), member, &mut missing);
            }
            if let Some(base) = shape.extends
                && !library.shapes.contains_key(base)
            {
                missing.push(format!("{}: {shape_name} extends {base}", label(library)));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}
