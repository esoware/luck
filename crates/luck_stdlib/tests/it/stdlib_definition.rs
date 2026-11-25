use luck_stdlib::{Stdlib, StdlibArgKind, StdlibDefinition, StdlibEntry, Variant};

fn definition(text: &str) -> StdlibDefinition {
    StdlibDefinition::parse(text).expect("the definition parses")
}

fn layered(variant: Variant, text: &str) -> Stdlib {
    Stdlib::new(variant).layer(&definition(text)).expect("the definition layers")
}

fn first_param_kind<'library>(
    stdlib: &'library Stdlib,
    path: &[&str],
) -> &'library StdlibArgKind<'library> {
    let Some(StdlibEntry::Function(function)) = stdlib.library().lookup(path) else {
        panic!("{path:?} should be a function");
    };
    &function.primary_signature().params[0].kind
}

#[test]
fn a_definition_declares_new_globals_with_their_members() {
    let stdlib = layered(
        Variant::Standalone,
        r#"
[globals.app]
kind = "namespace"

[globals.app.members.graphics]
kind = "namespace"

[globals.app.members.graphics.members.newImage]
kind = "function"
is_must_use = true
params = [{ kind = "string" }]
returns = "Image"

[shapes.Image.members.getWidth]
kind = "function"
is_method = true
"#,
    );
    let library = stdlib.library();
    assert_eq!(library.variant, Variant::Standalone);
    let Some(StdlibEntry::Function(new_image)) = library.lookup(["app", "graphics", "newImage"])
    else {
        panic!("app.graphics.newImage should be a function");
    };
    assert_eq!((new_image.min_args(), new_image.max_args()), (1, Some(1)));
    assert_eq!(new_image.returns_shape, Some("Image"));
    assert!(library.shape_member("Image", "getWidth").is_some());
    assert!(library.lookup(["print"]).is_some(), "the built-in library stays under it");
}

#[test]
fn a_namespace_over_a_namespace_keeps_the_members_it_leaves_alone() {
    let stdlib = layered(
        Variant::Standalone,
        r#"
[globals.string]
kind = "namespace"

[globals.string.members.trim]
kind = "function"
is_must_use = true
params = [{ kind = "string" }]
returns = "string"
"#,
    );
    let library = stdlib.library();
    assert!(library.lookup(["string", "trim"]).is_some());
    assert!(library.lookup(["string", "format"]).is_some());
    let Some(StdlibEntry::Function(method)) = library.shape_member("string", "trim") else {
        panic!("a string-first function of `string` is a method of strings");
    };
    assert!(method.is_method);
    assert_eq!(method.max_args(), Some(0));
    assert!(library.shape_member("string", "upper").is_some(), "the built-in methods stay");
}

#[test]
fn a_derived_string_method_keeps_a_deprecation_but_not_its_fix() {
    let stdlib = layered(
        Variant::Standalone,
        r#"
[globals.string]
kind = "namespace"

[globals.string.members.gfind]
kind = "function"
params = [{ kind = "string" }, { kind = "string" }]
deprecated = { message = "use string.gmatch", replacement = { template = "string.gmatch(%1)", is_exact = true } }
"#,
    );
    let Some(StdlibEntry::Function(method)) = stdlib.library().shape_member("string", "gfind")
    else {
        panic!("a string-first function of `string` is a method of strings");
    };
    let deprecation = method.deprecated.as_ref().expect("the method keeps the deprecation");
    assert_eq!(deprecation.message, "use string.gmatch");
    assert!(deprecation.replacement.is_none(), "a template numbers the receiver `%1`");
}

#[test]
fn a_later_definition_replaces_what_it_redeclares() {
    let stdlib = Stdlib::new(Variant::Standalone);
    let first = stdlib
        .layer(&definition(
            r#"
[globals.describe]
kind = "function"
params = [{ kind = "string" }, { kind = "function" }]
"#,
        ))
        .expect("the first layers");
    let second = first
        .layer(&definition(
            r#"
[globals.describe]
kind = "function"
params = [{ kind = "string" }, { kind = "function", is_required = false }]
"#,
        ))
        .expect("the second layers");
    let arity = |layered: &Stdlib| {
        let Some(StdlibEntry::Function(function)) = layered.library().lookup(["describe"]) else {
            panic!("describe should be a function");
        };
        function.min_args()
    };
    assert_eq!(arity(&first), 2, "a layer leaves the library it's over as it was");
    assert_eq!(arity(&second), 1);
    assert!(stdlib.library().lookup(["describe"]).is_none());
}

#[test]
fn a_member_added_to_a_base_shape_reaches_the_shapes_that_extend_it() {
    let stdlib = layered(
        Variant::Roblox,
        r#"
[shapes.Instance.members.GetTags]
kind = "function"
is_method = true
is_must_use = true

[shapes.Part]
extends = "Instance"

[shapes.Part.members.Anchored]
kind = "property"
is_read_only = false
"#,
    );
    let library = stdlib.library();
    assert!(library.lookup(["game", "GetTags"]).is_some(), "the DataModel extends Instance");
    assert!(library.shape_member("Part", "FindFirstChild").is_some());
    assert!(library.shape_member("Part", "Anchored").is_some());
    let members = library.shape_members("Part");
    assert_eq!(members.iter().filter(|(name, _)| *name == "Anchored").count(), 1);
    assert!(members.iter().any(|(name, _)| *name == "GetTags"));
}

#[test]
fn enums_join_the_enum_global_of_the_library_below() {
    let stdlib = layered(
        Variant::Roblox,
        r#"
[enums.Flavor]
items = ["Sweet", "Sour"]
"#,
    );
    let library = stdlib.library();
    let Some(StdlibEntry::Namespace(enum_global)) = library.lookup(["Enum"]) else {
        panic!("Enum should stay a namespace");
    };
    assert_eq!(enum_global.shape, Some("Enums"), "the built-in shape stays");
    assert!(library.lookup(["Enum", "Flavor", "Sour"]).is_some());
    assert!(library.lookup(["Enum", "KeyCode"]).is_some());
    assert!(library.lookup(["Enum", "Flavor", "GetEnumItems"]).is_some());
}

const SERVICE_WRAPPER: &str = r#"
[globals.Knit]
kind = "namespace"

[globals.Knit.members.GetService]
kind = "function"
params = [{ kind = "constant", set = "roblox_services" }]
"#;

#[test]
fn a_definition_names_a_constant_set_of_the_library_below() {
    let stdlib = layered(Variant::Roblox, SERVICE_WRAPPER);
    let StdlibArgKind::Constant(services) = first_param_kind(&stdlib, &["Knit", "GetService"])
    else {
        panic!("the param should take the service names");
    };
    assert!(services.iter().any(|service| service.value == "Players"));
}

#[test]
fn a_set_only_roblox_declares_leaves_its_param_any_string() {
    let stdlib = layered(Variant::Standalone, SERVICE_WRAPPER);
    assert!(matches!(first_param_kind(&stdlib, &["Knit", "GetService"]), StdlibArgKind::String));
}

#[test]
fn rejects_a_shape_or_set_no_library_declares() {
    let cases = [
        (
            r#"
[globals.open]
kind = "function"
returns = "Fiel"
"#,
            "`open` names the shape `Fiel`, which no library declares",
        ),
        (
            r#"
[shapes.Part]
extends = "Instanse"
"#,
            "`Part` names the shape `Instanse`, which no library declares",
        ),
        (
            r#"
[globals.ui]
kind = "namespace"

[globals.ui.members.pick]
kind = "function"
params = [{ kind = "constant", set = "colors" }]
"#,
            "`ui.pick` names the constant set `colors`, which no library declares",
        ),
    ];
    for ((text, expected), name) in
        cases.into_iter().zip([r#""Fiel""#, r#""Instanse""#, r#""colors""#])
    {
        let diag = Stdlib::new(Variant::Standalone).layer(&definition(text)).expect_err(expected);
        assert_eq!(diag.code, "E014");
        assert_eq!(diag.message, format!("invalid config: {expected}"));
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(name.as_bytes()));
    }
}

#[test]
fn a_definition_names_its_own_constant_set() {
    let stdlib = layered(
        Variant::Standalone,
        r#"
[constant_sets.modes]
values = ["fill", { value = "line", is_common = true }]

[globals.draw]
kind = "function"
params = [{ kind = "constant", set = "modes" }]
"#,
    );
    let StdlibArgKind::Constant(modes) = first_param_kind(&stdlib, &["draw"]) else {
        panic!("the param should take the set");
    };
    let values: Vec<&str> = modes.iter().map(|mode| mode.value).collect();
    assert_eq!(values, ["fill", "line"]);
    assert!(stdlib.library().constant_sets.contains_key("modes"));
}
