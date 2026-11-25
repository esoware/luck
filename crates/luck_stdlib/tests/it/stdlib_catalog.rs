use std::ptr;

use luck_allocator::Allocator;
use luck_ast::{Expression, Statement};
use luck_stdlib::{
    StdlibArgKind, StdlibConstant, StdlibEntry, StdlibFunction, StdlibLibrary, Variant, library_for,
};

use crate::common::{all_libraries, expand, parse, roblox, standalone};

#[test]
#[cfg(target_pointer_width = "64")]
fn stdlib_entry_is_two_words() {
    assert_eq!(size_of::<StdlibEntry<'_>>(), 16);
}

fn function_at<'library>(
    library: &'library StdlibLibrary<'library>,
    path: &[&str],
) -> &'library StdlibFunction<'library> {
    let Some(StdlibEntry::Function(function)) = library.lookup(path) else {
        panic!("{}: {path:?} should be a function", library.variant);
    };
    function
}

#[test]
fn every_library_declares_print_and_type() {
    for (variant, library) in all_libraries() {
        assert!(library.globals.contains_key("print"), "{variant} stdlib should expose print");
        assert!(library.globals.contains_key("type"), "{variant} stdlib should expose type");
    }
}

#[test]
fn every_variant_has_its_own_library() {
    for variant in Variant::ALL {
        let library = library_for(variant);
        assert_eq!(library.variant, variant);
        let others = Variant::ALL.into_iter().filter(|&other| other != variant);
        assert!(others.map(library_for).all(|other| !ptr::eq(library, other)), "{variant}");
    }
}

#[test]
fn table_getn_fixes_to_the_length_operator() {
    for library in [standalone(), roblox()] {
        let function = function_at(library, &["table", "getn"]);
        let deprecation = function.deprecated.as_ref().expect("table.getn must be deprecated");
        let replacement =
            deprecation.replacement.expect("table.getn deprecation must carry a replacement");
        assert_eq!(replacement.template, "#%1");
        assert!(
            !replacement.is_exact,
            "{}: `#` calls `__len`, which `table.getn` ignores",
            library.variant
        );
    }
}

#[test]
fn value_returning_functions_are_must_use() {
    for path in [
        &["pcall"][..],
        &["tostring"][..],
        &["tonumber"][..],
        &["type"][..],
        &["select"][..],
        &["table", "concat"][..],
        &["coroutine", "create"][..],
    ] {
        assert!(function_at(standalone(), path).is_must_use, "{path:?} should be must_use");
    }
}

#[test]
fn buffer_and_vector_present() {
    for library in [standalone(), roblox()] {
        assert!(library.lookup(&["buffer"]).is_some());
        assert!(library.lookup(&["buffer", "create"]).is_some());
        assert!(library.lookup(&["vector"]).is_some());
        assert!(library.lookup(&["vector", "create"]).is_some());
    }
    assert!(roblox().lookup(&["task", "spawn"]).is_some());
}

#[test]
fn the_rejected_integer_library_is_absent() {
    for library in [standalone(), roblox()] {
        assert!(library.lookup(&["integer"]).is_none());
        assert!(library.lookup(&["buffer", "readinteger"]).is_none());
        assert!(library.lookup(&["buffer", "writeinteger"]).is_none());
    }
}

#[test]
fn math_has_predicates_and_constants() {
    for library in [standalone(), roblox()] {
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
fn string_rep_has_no_separator() {
    for library in [standalone(), roblox()] {
        assert_eq!(function_at(library, &["string", "rep"]).max_args(), Some(2));
        assert_eq!(shape_function(library, "string", "rep").max_args(), Some(1));
    }
}

#[test]
fn buffer_bit_functions_present() {
    for library in [standalone(), roblox()] {
        for name in ["readbits", "writebits"] {
            assert!(library.lookup(&["buffer", name]).is_some(), "buffer.{name} missing");
        }
    }
}

#[test]
fn vector_create_wide_mode_is_standalone_only() {
    let standalone_function = function_at(standalone(), &["vector", "create"]);
    assert!(standalone_function.accepts_arg_count(3));
    assert!(standalone_function.accepts_arg_count(4), "wide mode takes w");

    let roblox_function = function_at(roblox(), &["vector", "create"]);
    assert!(roblox_function.accepts_arg_count(3));
    assert!(!roblox_function.accepts_arg_count(4), "roblox vectors are 3-wide");
}

#[test]
fn os_is_the_sandboxed_subset() {
    for library in [standalone(), roblox()] {
        assert!(library.lookup(&["os", "time"]).is_some());
        assert!(library.lookup(&["os", "clock"]).is_some());
        assert!(library.lookup(&["os", "getenv"]).is_none());
        assert!(library.lookup(&["os", "exit"]).is_none());
    }
}

#[test]
fn math_abs_is_pure_and_print_is_not() {
    assert!(function_at(standalone(), &["math", "abs"]).is_pure);
    assert!(!function_at(standalone(), &["print"]).is_pure);
}

#[test]
fn roblox_entries_absent_from_standalone() {
    let standalone_library = standalone();
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
            standalone_library.lookup(segments).is_none(),
            "{segments:?} must be absent from the standalone library"
        );
    }
}

#[test]
fn loadstring_present_on_both_variants() {
    assert!(standalone().lookup(&["loadstring"]).is_some(), "loadstring exists in the Luau CLI");
    assert!(
        roblox().lookup(&["loadstring"]).is_some(),
        "loadstring is a real Roblox global (gated by LoadStringEnabled)"
    );
}

#[test]
fn roblox_only_deprecations_stay_off_standalone() {
    for name in ["collectgarbage", "getfenv", "setfenv"] {
        let standalone_entry =
            standalone().lookup(&[name]).unwrap_or_else(|| panic!("{name} on standalone"));
        assert!(
            standalone_entry.deprecation().is_none(),
            "{name} must not be deprecated on standalone"
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
    let replacement = deprecation.replacement.expect("ypcall fixes to pcall");
    assert_eq!((replacement.template, replacement.is_exact), ("pcall(%1)", true));
}

#[test]
fn roblox_task_replacements_change_scheduling() {
    for name in ["wait", "spawn", "delay"] {
        let deprecation =
            function_at(roblox(), &[name]).deprecated.as_ref().expect("legacy scheduling");
        let replacement = deprecation.replacement.expect("a `task` replacement");
        assert!(
            !replacement.is_exact,
            "`{name}` runs on the legacy scheduler, so `{}` changes when the code runs",
            replacement.template
        );
    }
}

#[test]
fn core_luau_entries_on_both_variants() {
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
        &["bit32"],
        &["bit32", "byteswap"],
        &["bit32", "countlz"],
        &["bit32", "countrz"],
        &["utf8"],
        &["vector"],
        &["buffer"],
    ];
    for segments in checks {
        for library in [standalone(), roblox()] {
            assert!(
                library.lookup(*segments).is_some(),
                "{segments:?} missing from {}",
                library.variant
            );
        }
    }
}

#[test]
fn allowed_constants_need_every_matching_signature_constrained() {
    let function = function_at(roblox(), &["BrickColor", "new"]);
    let names: Vec<_> =
        function.allowed_constants(0, 1).expect("the 1-arg form takes a BrickColor name").collect();
    assert_eq!(names.len(), 1);
    assert!(names[0].iter().any(|constant| constant.value == "Bright red"));
    assert!(function.allowed_constants(0, 3).is_none(), "the 3-arg form takes numbers");
    assert!(function.allowed_constants(0, 9).is_none(), "no signature takes 9");

    let tostring = function_at(standalone(), &["tostring"]);
    assert!(tostring.allowed_constants(0, 1).is_none());
}

#[test]
fn roblox_datatypes_present_with_representative_statics() {
    let roblox_library = roblox();
    let standalone_library = standalone();
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
            standalone_library.lookup(segments).is_none(),
            "{segments:?} must be absent from the standalone library"
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
    for n in [0, 1, 2, 3, 7, 12] {
        assert!(function.accepts_arg_count(n), "CFrame.new must accept {n} args");
    }
    for n in [4, 5, 6, 8, 11, 13] {
        assert!(!function.accepts_arg_count(n), "CFrame.new must reject {n} args");
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
    for n in [1, 3, 5, 6] {
        assert!(function.accepts_arg_count(n), "PhysicalProperties.new must accept {n} args");
    }
    for n in [0, 2, 4, 7] {
        assert!(!function.accepts_arg_count(n), "PhysicalProperties.new must reject {n} args");
    }
}

fn shape_function<'library>(
    library: &'library StdlibLibrary<'library>,
    shape: &str,
    name: &str,
) -> &'library StdlibFunction<'library> {
    let Some(StdlibEntry::Function(function)) = library.shape_member(shape, name) else {
        panic!("{}: shape {shape} should have function member {name}", library.variant);
    };
    function
}

#[test]
fn string_receiver_shape_derived_in_every_library() {
    for (variant, library) in all_libraries() {
        let upper = shape_function(library, "string", "upper");
        assert!(upper.is_method);
        assert_eq!(upper.min_args(), 0, "subject param dropped");
        assert_eq!(upper.returns_shape, Some("string"));
        let find = shape_function(library, "string", "find");
        assert!(find.min_args() >= 1, "pattern still required");
        for excluded in ["char", "dump"] {
            assert!(
                library.shape_member("string", excluded).is_none(),
                "{variant}: string.{excluded} is not subject-first"
            );
        }
    }
}

fn constant_set<'library>(
    library: &'library StdlibLibrary<'library>,
    segments: &[&str],
    param_index: usize,
) -> Option<&'library [StdlibConstant<'library>]> {
    let params = &function_at(library, segments).primary_signature().params;
    let StdlibArgKind::Constant(values) = &params.get(param_index)?.kind else {
        return None;
    };
    Some(values)
}

fn find<'values>(
    values: &'values [StdlibConstant<'values>],
    name: &str,
) -> Option<&'values StdlibConstant<'values>> {
    values.iter().find(|constant| constant.value == name)
}

#[test]
fn game_is_a_shaped_datamodel() {
    let get_service = function_at(roblox(), &["game", "GetService"]);
    assert!(get_service.is_method);
    assert!(get_service.is_must_use);
    assert_eq!(get_service.returns_shape, Some("Instance"));
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
    assert!(standalone().lookup(&["game"]).is_none());
}

#[test]
fn get_service_takes_every_service_game_can_reach() {
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
fn instance_new_takes_creatable_classes_and_is_a_every_class() {
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
fn brickcolor_new_takes_the_204_brickcolor_names() {
    let function = function_at(roblox(), &["BrickColor", "new"]);
    let StdlibArgKind::Constant(names) = &function.signatures[0].params[0].kind else {
        panic!("BrickColor.new arg 0 should be the name set");
    };
    assert_eq!(names.len(), 204);
    assert!(find(names, "Bright red").is_some());
    assert!(find(names, "Really black").is_some());
}

#[test]
fn settings_providers_get_service_takes_their_own_services() {
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

#[test]
fn enum_global_holds_every_enum_type() {
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
    assert_eq!(enum_global.shape, Some("Enums"));
}

#[test]
fn enum_items_are_read_only_enum_item_constants() {
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
        assert_eq!(value.shape, Some("EnumItem"));
    }
}

#[test]
fn enum_types_are_enum_shaped() {
    let StdlibEntry::Namespace(material) =
        roblox().lookup(&["Enum", "Material"]).expect("Enum.Material")
    else {
        panic!("Enum.Material should be a namespace");
    };
    assert_eq!(material.shape, Some("Enum"));
}

#[test]
fn deprecated_enum_items_carry_a_deprecation() {
    let entry =
        roblox().lookup(&["Enum", "AlignType", "Parallel"]).expect("Enum.AlignType.Parallel");
    assert!(entry.deprecation().is_some(), "AlignType.Parallel is tagged Deprecated in the dump");
}

#[test]
fn enum_item_properties_resolve_through_its_shape() {
    for member in ["Name", "Value", "EnumType"] {
        assert!(
            roblox().lookup(&["Enum", "Material", "Grass", member]).is_some(),
            "EnumItem member {member} should resolve through the shape"
        );
    }
}

#[test]
fn standalone_has_no_enum() {
    assert!(standalone().lookup(&["Enum"]).is_none());
}

#[test]
fn standalone_collectgarbage_accepts_collect() {
    let options = constant_set(standalone(), &["collectgarbage"], 0)
        .expect("collectgarbage's option is a constant set");
    assert!(
        find(options, "collect").is_some(),
        "the Luau CLI's collectgarbage accepts collect, and it is the default"
    );
}

#[test]
fn standalone_collectgarbage_is_not_must_use() {
    assert!(
        !function_at(standalone(), &["collectgarbage"]).is_must_use,
        "collectgarbage() runs a collection and returns nothing in the Luau CLI"
    );
}

#[test]
fn math_randomseed_requires_a_seed() {
    for library in [standalone(), roblox()] {
        assert_eq!(
            function_at(library, &["math", "randomseed"]).min_args(),
            1,
            "math.randomseed calls luaL_checkinteger on its seed, so it can't be omitted"
        );
    }
}

#[test]
fn vector_create_accepts_two_components() {
    assert!(
        function_at(standalone(), &["vector", "create"]).accepts_arg_count(2),
        "the 2-component constructor RFC is implemented: vector.create(x, y) zeroes z"
    );
}

#[test]
fn vector_lerp_is_cataloged() {
    for library in [standalone(), roblox()] {
        assert!(
            matches!(library.lookup(&["vector", "lerp"]), Some(StdlibEntry::Function(_))),
            "{}: vector.lerp is an implemented RFC and in the Roblox docs",
            library.variant
        );
    }
}

#[test]
fn coroutine_finally_is_cataloged() {
    for library in [standalone(), roblox()] {
        assert!(
            matches!(library.lookup(&["coroutine", "finally"]), Some(StdlibEntry::Function(_))),
            "{}: coroutine.finally is in the Luau VM and the Roblox docs",
            library.variant
        );
    }
}

#[test]
fn tostring_is_not_pure() {
    for (variant, library) in all_libraries() {
        assert!(
            !function_at(library, &["tostring"]).is_pure,
            "{variant}: tostring runs the __tostring metamethod, so it isn't pure"
        );
    }
}

#[test]
fn unpack_is_not_deprecated() {
    for library in [standalone(), roblox()] {
        assert!(
            function_at(library, &["unpack"]).deprecated.is_none(),
            "{}: luau.org documents unpack as equivalent to table.unpack, and neither it nor \
             the Roblox globals page marks it deprecated",
            library.variant
        );
    }
}

fn fix_template<'library>(
    library: &'library StdlibLibrary<'library>,
    path: &[&str],
) -> Option<&'library str> {
    Some(function_at(library, path).deprecated.as_ref()?.replacement?.template)
}

/// Whether `text`, read as a returned value, is what `is_expected` says.
fn returns<F>(text: &str, is_expected: F) -> bool
where
    F: FnOnce(&Expression<'_>) -> bool,
{
    let source = format!("return {text}");
    let allocator = Allocator::new();
    let chunk = parse(&allocator, &source);
    let [Statement::Return(return_stmt)] = &*chunk.stmts else {
        panic!("{text} must parse as a return");
    };
    is_expected(return_stmt.exprs.first().expect("one returned value"))
}

#[test]
fn table_getn_fix_groups_a_compound_operand() {
    let template = fix_template(standalone(), &["table", "getn"]).expect("table.getn has a fix");
    let replacement = expand(template, &["a or b"]);
    assert!(
        returns(&replacement, |expr| matches!(expr, Expression::UnaryOp(_))),
        "table.getn(a or b) takes the length of `a or b`, but `{replacement}` takes the length \
         of a alone"
    );
}

#[test]
fn ypcall_fix_keeps_the_forwarded_arguments() {
    let Some(template) = fix_template(roblox(), &["ypcall"]) else {
        return;
    };
    let replacement = expand(template, &["callback", "forwarded"]);
    assert!(
        replacement.contains("forwarded"),
        "ypcall(f, x) calls f with x, so the fix can't drop x: `{replacement}`"
    );
}
