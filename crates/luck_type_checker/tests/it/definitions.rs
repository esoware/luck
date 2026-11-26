use luck_allocator::Allocator;
use luck_type_checker::builtin_definitions;

use crate::common::{check, checker, described};

#[test]
fn definitions_loaded_from_text_declare_globals() {
    let main = "\
--!strict
local greeting: number = greet(\"world\")
return greeting
";
    let mut checker = checker(&[("main", main)]);

    checker.load_definitions("@app", "declare function greet(name: string): string\n").unwrap();
    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(
        described(&diags[0], main),
        [("E015", "Expected this to be 'number', but got 'string'".to_owned(), "greet(\"world\")")]
    );
}

#[test]
fn definitions_that_dont_parse_report_e008_in_their_text() {
    let mut checker = checker(&[]);
    let source = "declare function greet(name: string: string\n";

    let diags = checker.load_definitions("@app", source).unwrap_err();

    assert_eq!(diags[0].code, "E008", "{diags:?}");
    assert!(diags[0].span.is_some_and(|span| span.end as usize <= source.len()), "{diags:?}");
}

#[test]
fn definitions_with_a_type_error_report_e015_and_load_nothing() {
    let main = "\
--!strict
return greet
";
    let mut checker = checker(&[("main", main)]);
    let source = "declare function greet(name: Missing): string\n";

    let diags = checker.load_definitions("@app", source).unwrap_err();
    let after = check(&mut checker, &["main"]).unwrap();

    assert_eq!(described(&diags, source)[0].0, "E015", "{diags:?}");
    assert_eq!(described(&diags, source)[0].2, "Missing", "{diags:?}");
    assert_eq!(after[0].len(), 1, "{after:?}");
}

#[test]
fn the_builtin_definitions_are_luaus_and_luck_parses_them() {
    let source = builtin_definitions();

    assert!(source.contains("declare function print"), "{source}");
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
}

const ROBLOX: &str = "
declare extern type Instance with
    Name: string
end
declare extern type Part extends Instance with end
declare extern type Players extends Instance with end
declare extern type ServiceProvider extends Instance with
    function FindService(self, className: string): Instance
    function GetService(self, className: string): Instance
end
declare Instance: { new: (className: string) -> Instance }
declare game: ServiceProvider

declare extern type EnumItem with
    Name: string
end
declare extern type EnumMaterial extends EnumItem with end
declare extern type EnumFont extends EnumItem with end
declare Enum: { Material: { Plastic: EnumMaterial }, Font: { Arial: EnumFont } }
";

const ROBLOX_MAIN: &str = r#"--!strict
local part: Part = Instance.new("Part")
local players: Players = game:GetService("Players")
local found: Players = game:FindService("Players")
local typo = Instance.new("Parts")
local item = Instance.new("EnumItem")
local function make(className: string): Instance
    return Instance.new(className)
end
local plastic: Enum.Material = Enum.Material.Plastic
local arial: Enum.Material = Enum.Font.Arial
return part, players, found, typo, item, make, plastic, arial
"#;

/// `Parts` names no type, and `EnumItem` one that isn't an `Instance`.
#[test]
fn roblox_studio_types_a_class_a_string_names_and_binds_enum() {
    let mut checker = checker(&[("main", ROBLOX_MAIN)]);
    checker.load_definitions("@roblox", ROBLOX).unwrap();
    checker.type_as_roblox_studio();

    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(
        described(&diags[0], ROBLOX_MAIN),
        [
            ("E015", "Unknown type 'Parts'".to_owned(), "\"Parts\""),
            ("E015", "Unknown type 'EnumItem'".to_owned(), "\"EnumItem\""),
            (
                "E015",
                "Expected this to be 'Enum.Material', but got 'Enum.Font'".to_owned(),
                "Enum.Font.Arial"
            ),
        ]
    );
}

#[test]
fn roblox_definitions_alone_name_no_class_by_a_string_and_bind_no_enum() {
    let mut checker = checker(&[("main", ROBLOX_MAIN)]);
    checker.load_definitions("@roblox", ROBLOX).unwrap();

    let diags = check(&mut checker, &["main"]).unwrap();

    let messages: Vec<String> =
        described(&diags[0], ROBLOX_MAIN).into_iter().map(|(_, message, _)| message).collect();
    assert!(
        messages.contains(&"Expected this to be 'Part', but got 'Instance'".to_owned()),
        "{messages:?}"
    );
    assert!(messages.contains(&"Unknown type 'Enum.Material'".to_owned()), "{messages:?}");
}
