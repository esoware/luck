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

const SHAPES: &str = "
declare extern type Shape with
    Sides: number
end
declare extern type Circle extends Shape with end
declare extern type Square extends Shape with end
declare extern type Registry with
    function Get(self, kind: string): Shape
end
declare Shapes: { new: (kind: string) -> Shape }
declare registry: Registry

declare extern type ColorItem with
    Name: string
end
declare extern type ColorRed extends ColorItem with end
declare extern type ColorBlue extends ColorItem with end
declare Color: { Red: ColorRed, Blue: ColorBlue }
";

const SHAPES_MAIN: &str = r#"--!strict
local circle: Circle = Shapes.new("Circle")
local square: Square = registry:Get("Square")
local triangle = Shapes.new("Triangle")
local color = Shapes.new("ColorItem")
local function make(kind: string): Shape
    return Shapes.new(kind)
end
local red: Color.Red = Color.Red
local blue: Color.Blue = Color.Red
return circle, square, triangle, color, make, red, blue
"#;

/// `Triangle` names no type, and `ColorItem` one that isn't a `Shape`.
#[test]
fn hooks_return_the_type_a_string_names_and_bind_a_type_namespace() {
    let mut checker = checker(&[("main", SHAPES_MAIN)]);
    checker.load_definitions("@shapes", SHAPES).unwrap();
    checker.return_named_type("Shapes", "new");
    checker.return_named_type("Registry", "Get");
    checker.bind_type_namespace("Color", "ColorItem");

    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(
        described(&diags[0], SHAPES_MAIN),
        [
            ("E015", "Unknown type 'Triangle'".to_owned(), "\"Triangle\""),
            ("E015", "Unknown type 'ColorItem'".to_owned(), "\"ColorItem\""),
            (
                "E015",
                "Expected this to be 'Color.Blue', but got 'Color.Red'".to_owned(),
                "Color.Red"
            ),
        ]
    );
}

#[test]
fn definitions_alone_name_no_type_by_a_string_and_bind_no_namespace() {
    let mut checker = checker(&[("main", SHAPES_MAIN)]);
    checker.load_definitions("@shapes", SHAPES).unwrap();

    let diags = check(&mut checker, &["main"]).unwrap();

    let messages: Vec<String> =
        described(&diags[0], SHAPES_MAIN).into_iter().map(|(_, message, _)| message).collect();
    assert!(
        messages.contains(&"Expected this to be 'Circle', but got 'Shape'".to_owned()),
        "{messages:?}"
    );
    assert!(messages.contains(&"Unknown type 'Color.Red'".to_owned()), "{messages:?}");
}
