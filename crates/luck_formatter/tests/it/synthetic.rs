//! Each tree is built in code, so the formatter has no source text to lean
//! on, as it always has in the source-based tests.

use luck_ast::{
    Block, Expression, FieldSpec, FunctionSignature, InterpolatedStringPart, LastStatement,
    MAX_NESTING_DEPTH, Synth, SyntheticComment, TypeFieldAccess, TypeFieldSpec,
    block_nesting_depth,
};
use luck_formatter::{CollapseSimpleStatement, Comments, FormatOptions, format_block};
use luck_token::{BinOp, LuaVersion, TokenKind, UnOp, decode_string};

use crate::common::assert_format_with;

/// The output also has to format as itself and re-parse clean under
/// `version`.
fn assert_format_block_with(
    block: &Block,
    comments: Comments,
    version: LuaVersion,
    options: &FormatOptions,
    expected: &str,
) {
    let output = format_block(block, comments, options).expect("the block nests within the cap");
    assert_eq!(output, expected, "format mismatch");
    assert_format_with(expected, expected, version, options);
}

fn assert_format_block(block: &Block, version: LuaVersion, expected: &str) {
    assert_format_block_with(block, Comments::none(), version, &FormatOptions::default(), expected);
}

#[test]
fn long_string_delimiters_and_bytes_roundtrip() {
    let synth = Synth::new();
    let mut cases = vec![
        (String::new(), "return [[]]\n".to_owned()),
        ("]]=".to_owned(), "return [==[]]=]==]\n".to_owned()),
        ("a\0b".to_owned(), "return \"a\\0b\"\n".to_owned()),
        ("\n\0\r".to_owned(), "return \"\\n\\0\\r\"\n".to_owned()),
    ];
    // This content would close every level up to `level`, so it needs the
    // one after.
    for level in 0..8 {
        let mut content = String::new();
        for preceding in 0..level {
            content.push(']');
            content.push_str(&"=".repeat(preceding));
            content.push(']');
        }
        content.push(']');
        content.push_str(&"=".repeat(level));
        let equals = "=".repeat(level.saturating_add(1));
        let expected = format!("return [{equals}[{content}]{equals}]\n");
        cases.push((content, expected));
    }
    for (content, expected) in cases {
        let block = synth.block(vec![], Some(synth.return_(vec![synth.long_string(&content)])));
        for version in LuaVersion::ALL {
            assert_format_block(&block, version, &expected);
            let lexed = luck_lexer::lex(&expected, version);
            let literal = lexed
                .tokens
                .iter()
                .find_map(|token| {
                    if let TokenKind::StringLiteral(text) = &token.kind { Some(text) } else { None }
                })
                .expect("string literal");
            assert_eq!(
                decode_string(literal, version).as_deref(),
                Some(content.as_bytes()),
                "{version:?}: {expected}"
            );
        }
    }
}

/// For each child of `composite_types_preserve_grouping`, its layouts at width
/// 1 and at widths 60, 80, and 120 as an optional's operand and as either
/// operand of a union and of an intersection.
const COMPOSITE_TYPE_LAYOUTS: [[(&str, &str); 5]; 4] = [
    [
        (
            "\
type Result = (
\t| A
\t| B)?
",
            "type Result = (A | B)?\n",
        ),
        (
            "\
type Result =
\t| (
\t\t| A
\t\t| B)
\t| C
",
            "type Result = (A | B) | C\n",
        ),
        (
            "\
type Result =
\t| C
\t| (
\t\t| A
\t\t| B)
",
            "type Result = C | (A | B)\n",
        ),
        (
            "\
type Result =
\t& (
\t\t| A
\t\t| B)
\t& C
",
            "type Result = (A | B) & C\n",
        ),
        (
            "\
type Result =
\t& C
\t& (
\t\t| A
\t\t| B)
",
            "type Result = C & (A | B)\n",
        ),
    ],
    [
        (
            "\
type Result = (
\t& A
\t& B)?
",
            "type Result = (A & B)?\n",
        ),
        (
            "\
type Result =
\t| (
\t\t& A
\t\t& B)
\t| C
",
            "type Result = (A & B) | C\n",
        ),
        (
            "\
type Result =
\t| C
\t| (
\t\t& A
\t\t& B)
",
            "type Result = C | (A & B)\n",
        ),
        (
            "\
type Result =
\t& (
\t\t& A
\t\t& B)
\t& C
",
            "type Result = (A & B) & C\n",
        ),
        (
            "\
type Result =
\t& C
\t& (
\t\t& A
\t\t& B)
",
            "type Result = C & (A & B)\n",
        ),
    ],
    [
        ("type Result = (() -> A)?\n", "type Result = (() -> A)?\n"),
        (
            "\
type Result =
\t| (() -> A)
\t| C
",
            "type Result = (() -> A) | C\n",
        ),
        (
            "\
type Result =
\t| C
\t| (() -> A)
",
            "type Result = C | (() -> A)\n",
        ),
        (
            "\
type Result =
\t& (() -> A)
\t& C
",
            "type Result = (() -> A) & C\n",
        ),
        (
            "\
type Result =
\t& C
\t& (() -> A)
",
            "type Result = C & (() -> A)\n",
        ),
    ],
    [
        ("type Result = A??\n", "type Result = A??\n"),
        (
            "\
type Result =
\t| A?
\t| C
",
            "type Result = A? | C\n",
        ),
        (
            "\
type Result =
\t| C
\t| A?
",
            "type Result = C | A?\n",
        ),
        (
            "\
type Result =
\t& (A?)
\t& C
",
            "type Result = (A?) & C\n",
        ),
        (
            "\
type Result =
\t& C
\t& (A?)
",
            "type Result = C & (A?)\n",
        ),
    ],
];

#[test]
fn composite_types_preserve_grouping() {
    let synth = Synth::new();
    let children = [
        synth.ty_union(vec![synth.ty_named("A"), synth.ty_named("B")]),
        synth.ty_intersection(vec![synth.ty_named("A"), synth.ty_named("B")]),
        synth.ty_function(vec![], synth.ty_named("A")),
        synth.ty_optional(synth.ty_named("A")),
    ];
    for (child, layouts) in children.into_iter().zip(COMPOSITE_TYPE_LAYOUTS) {
        let types = [
            synth.ty_optional(child.clone()),
            synth.ty_union(vec![child.clone(), synth.ty_named("C")]),
            synth.ty_union(vec![synth.ty_named("C"), child.clone()]),
            synth.ty_intersection(vec![child.clone(), synth.ty_named("C")]),
            synth.ty_intersection(vec![synth.ty_named("C"), child]),
        ];
        for (type_value, (narrowest, wider)) in types.into_iter().zip(layouts) {
            let block =
                synth.block(vec![synth.type_declaration(false, "Result", None, type_value)], None);
            for (line_width, expected) in [(1, narrowest), (60, wider), (80, wider), (120, wider)] {
                let options = FormatOptions { line_width, ..FormatOptions::default() };
                let comments = Comments::none();
                assert_format_block_with(&block, comments, LuaVersion::Luau, &options, expected);
            }
        }
    }
    let optional_function = synth.ty_optional(synth.ty_function(vec![], synth.ty_named("A")));
    let block =
        synth.block(vec![synth.type_declaration(false, "Callback", None, optional_function)], None);
    assert_format_block(&block, LuaVersion::Luau, "type Callback = (() -> A)?\n");
}

#[test]
fn typed_local_roundtrips() {
    let synth = Synth::new();
    let optional = synth.ty_optional(synth.ty_named("number"));
    let stmt = synth
        .local_full(vec![synth.attributed_name("value", Some(optional), None)], vec![synth.nil()]);
    let block = synth.block(vec![stmt], None);
    assert_format_block(&block, LuaVersion::Luau, "local value: number? = nil\n");
}

#[test]
fn typed_function_roundtrips() {
    let synth = Synth::new();
    let signature = FunctionSignature {
        params: vec![synth.param_typed("n", synth.ty_named("number"))],
        return_type: Some(synth.ty_named("number")),
        ..FunctionSignature::default()
    };
    let return_stmt = synth.return_(vec![synth.name_expr("n")]);
    let function_def =
        synth.function_def_full(Vec::new(), signature, synth.block(vec![], Some(return_stmt)));
    let local = synth.local(&["identity"], vec![function_def]);
    let block = synth.block(vec![local], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
local identity = function(n: number): number
\treturn n
end
",
    );
}

#[test]
fn mixed_stmts_roundtrip() {
    let synth = Synth::new();

    // local data = { 1, name = 2, ["k"] = true }
    let table = synth.table(vec![
        FieldSpec::Positional(synth.number("1")),
        FieldSpec::Named("name", synth.number("2")),
        FieldSpec::Bracketed(synth.string("k"), synth.boolean(true)),
    ]);
    let local_data = synth.local(&["data"], vec![table]);

    // for key, value in pairs(data) do end
    let pairs_call = synth.call(synth.name_expr("pairs"), vec![synth.name_expr("data")]);
    let generic_for = synth.generic_for(
        vec![synth.param("key"), synth.param("value")],
        vec![pairs_call],
        synth.block(vec![], None),
    );

    // if data then data:insert(3) elseif false then else end
    let method = synth.method_call(synth.name_expr("data"), "insert", vec![synth.number("3")]);
    let if_stmt = synth.if_(
        synth.name_expr("data"),
        synth.block(vec![synth.call_stmt(method)], None),
        vec![(synth.boolean(false), synth.block(vec![], None))],
        Some(synth.block(vec![], None)),
    );

    let block = synth.block(vec![local_data, generic_for, if_stmt], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
local data = { 1, name = 2, [\"k\"] = true }
for key, value in pairs(data) do
end
if data then
\tdata:insert(3)
elseif false then
else
end
",
    );
}

#[test]
fn precedence_parens_roundtrip() {
    let synth = Synth::new();
    // (a + b) * c
    let grouped_sum = synth.binary_op(
        synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b")),
        BinOp::Mul,
        synth.name_expr("c"),
    );
    // a - (b - c)
    let right_sub = synth.binary_op(
        synth.name_expr("a"),
        BinOp::Sub,
        synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c")),
    );
    // (-a) ^ b
    let unary_power = synth.binary_op(
        synth.unary_op(UnOp::Neg, synth.name_expr("a")),
        BinOp::Pow,
        synth.name_expr("b"),
    );
    // (a .. b) .. c
    let left_concat = synth.binary_op(
        synth.binary_op(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b")),
        BinOp::Concat,
        synth.name_expr("c"),
    );
    let return_stmt = synth.return_(vec![grouped_sum, right_sub, unary_power, left_concat]);
    let block = synth.block(vec![], Some(return_stmt));
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "return (a + b) * c, a - (b - c), (-a) ^ b, (a .. b) .. c\n",
    );
}

#[test]
fn if_expr_operand_roundtrips() {
    let synth = Synth::new();
    // a or (if c then 1 else 2): unparenthesized, the if-expression's else
    // branch would swallow everything after it.
    let if_expr = synth.if_expr(synth.name_expr("c"), synth.number("1"), vec![], synth.number("2"));
    let guarded = synth.binary_op(synth.name_expr("a"), BinOp::Or, if_expr);
    let block = synth.block(vec![], Some(synth.return_(vec![guarded])));
    assert_format_block(&block, LuaVersion::Luau, "return a or (if c then 1 else 2)\n");
}

#[test]
fn prefix_wrapping_roundtrips() {
    let synth = Synth::new();
    // ("s"):rep(2) and ({}).field both need their receivers parenthesized.
    let string_method = synth.method_call(synth.string("s"), "rep", vec![synth.number("2")]);
    let table_field = synth.field(synth.table(vec![]), "field");
    let block = synth.block(vec![], Some(synth.return_(vec![string_method, table_field])));
    assert_format_block(&block, LuaVersion::Luau, "return (\"s\"):rep(2), ({}).field\n");
}

#[test]
fn field_or_index_roundtrips() {
    let synth = Synth::new();
    let good = synth.field_or_index(synth.name_expr("t"), "ok");
    let bad = synth.field_or_index(synth.name_expr("t"), "not ok");
    let keyword = synth.field_or_index(synth.name_expr("t"), "end");
    let block = synth.block(vec![], Some(synth.return_(vec![good, bad, keyword])));
    assert_format_block(&block, LuaVersion::Luau, "return t.ok, t[\"not ok\"], t[\"end\"]\n");
}

#[test]
fn string_bytes_roundtrips() {
    let synth = Synth::new();
    let bytes = synth.string_bytes(&[0xff, 0x00, b'a', b'1']);
    let block = synth.block(vec![synth.local(&["blob"], vec![bytes])], None);
    assert_format_block(&block, LuaVersion::Luau, "local blob = \"\\255\\0a1\"\n");
}

#[test]
fn long_string_roundtrips() {
    let synth = Synth::new();
    let text = synth.long_string(
        "\
line one
line ]] two",
    );
    let block = synth.block(vec![synth.local(&["doc"], vec![text])], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
local doc = [=[line one
line ]] two]=]
",
    );
}

#[test]
fn numeric_specials_roundtrip() {
    let synth = Synth::new();
    let values = vec![
        synth.number_f64(3.0),
        synth.number_f64(-2.5),
        synth.number_f64(f64::INFINITY),
        synth.number_f64(f64::NAN),
        synth.number_i64(-42),
        synth.number_i64(i64::MIN),
    ];
    let block = synth.block(vec![], Some(synth.return_(vec![synth.array(values)])));
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "return { 3.0, -2.5, 1 / 0, 0 / 0, -42, 0x8000000000000000 }\n",
    );
}

#[test]
fn call_sugar_roundtrips() {
    let synth = Synth::new();
    let require = synth.call_string(synth.name_expr("require"), "module");
    let configure = synth.call_table(
        synth.name_expr("configure"),
        vec![FieldSpec::Named("debug", synth.boolean(true))],
    );
    let block = synth.block(vec![synth.call_stmt(require), synth.call_stmt(configure)], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
require(\"module\")
configure({ debug = true })
",
    );
}

#[test]
fn attributed_local_roundtrips_in_lua54() {
    let synth = Synth::new();
    let stmt = synth.local_full(
        vec![synth.attributed_name("frozen", None, Some("const"))],
        vec![synth.number("1")],
    );
    let block = synth.block(vec![stmt], None);
    assert_format_block(&block, LuaVersion::Lua54, "local frozen <const> = 1\n");
}

#[test]
fn globals_roundtrip_in_lua55() {
    let synth = Synth::new();
    let global_declaration =
        synth.global_declaration(vec![synth.attributed_name("shared", None, None)], vec![]);
    let global_function =
        synth.global_function("main", FunctionSignature::default(), synth.block(vec![], None));
    let global_star = synth.global_star(None);
    let block = synth.block(vec![global_declaration, global_function, global_star], None);
    assert_format_block(
        &block,
        LuaVersion::Lua55,
        "\
global shared

global function main() end

global *
",
    );
}

#[test]
fn mid_block_break_roundtrips_in_lua54() {
    let synth = Synth::new();
    let conditional_break = synth.if_(
        synth.name_expr("done"),
        synth.block(vec![synth.break_stmt()], None),
        vec![],
        None,
    );
    let step = synth.call_stmt(synth.call(synth.name_expr("step"), vec![]));
    let loop_stmt =
        synth.while_(synth.boolean(true), synth.block(vec![conditional_break, step], None));
    let block = synth.block(vec![loop_stmt], None);
    assert_format_block(
        &block,
        LuaVersion::Lua54,
        "\
while true do
\tif done then
\t\tbreak
\tend
\tstep()
end
",
    );
}

#[test]
fn goto_roundtrips_in_lua54() {
    let synth = Synth::new();
    let block = synth.block(vec![synth.goto_("done"), synth.label("done")], None);
    assert_format_block(
        &block,
        LuaVersion::Lua54,
        "\
goto done
::done::
",
    );
}

#[test]
fn luau_type_forms_roundtrip() {
    let synth = Synth::new();

    // export type Handler<T...> = <U>(name: string, T...) -> (U, boolean)
    let generics = synth.generic_type_list(vec![("T", true)]);
    let function_generics = synth.generic_type_list(vec![("U", false)]);
    let function_ty = synth.ty_function_full(
        Some(function_generics),
        vec![(Some("name"), synth.ty_named("string")), (None, synth.ty_generic_pack("T"))],
        synth.ty_pack(vec![synth.ty_named("U"), synth.ty_named("boolean")]),
    );
    let alias = synth.type_declaration(true, "Handler", Some(generics), function_ty);

    // type Entry = { read id: number, [string]: boolean }
    let table_ty = synth.ty_table(vec![
        TypeFieldSpec::Named {
            access: Some(TypeFieldAccess::Read),
            name: "id",
            value: synth.ty_named("number"),
        },
        TypeFieldSpec::Indexer {
            access: None,
            key: synth.ty_named("string"),
            value: synth.ty_named("boolean"),
        },
    ]);
    let entry = synth.type_declaration(false, "Entry", None, table_ty);

    // type Mode = "fast" | true | nil
    let mode = synth.type_declaration(
        false,
        "Mode",
        None,
        synth.ty_union(vec![
            synth.ty_singleton_string("fast"),
            synth.ty_singleton_bool(true),
            synth.ty_singleton_nil(),
        ]),
    );

    // local narrowed = value :: typeof(template)
    let cast =
        synth.type_cast(synth.name_expr("value"), synth.ty_typeof(synth.name_expr("template")));
    let narrowed = synth.local(&["narrowed"], vec![cast]);

    let block = synth.block(vec![alias, entry, mode, narrowed], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
export type Handler<T...> = <U>(name: string, T...) -> (U, boolean)

type Entry = { read id: number, [string]: boolean }

type Mode = \"fast\" | true | nil

local narrowed = value :: typeof(template)
",
    );
}

#[test]
fn function_attributes_roundtrip() {
    let synth = Synth::new();
    let stmt = synth.local_function_full(
        "hot",
        vec![synth.function_attribute("native", None)],
        FunctionSignature::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![stmt], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
@native
local function hot() end
",
    );
}

#[test]
fn attribute_args_roundtrip() {
    let synth = Synth::new();
    let args = synth.table(vec![FieldSpec::Named("use", synth.string("y"))]);
    let attribute = synth.function_attribute("deprecated", Some(vec![args]));
    let stmt = synth.local_function_full(
        "old",
        vec![attribute],
        FunctionSignature::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![stmt], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
@[deprecated({ use = \"y\" })]
local function old() end
",
    );
}

#[test]
fn cast_before_comparison_roundtrips() {
    let synth = Synth::new();
    // Bare `value :: number < limit` fails to parse: the type grammar reads
    // `<` as generic arguments. The synthesized form must carry parens.
    let cast = synth.type_cast(synth.name_expr("value"), synth.ty_named("number"));
    let compared = synth.binary_op(cast, BinOp::Lt, synth.name_expr("limit"));
    let block = synth.block(vec![], Some(synth.return_(vec![compared])));
    assert_format_block(&block, LuaVersion::Luau, "return (value :: number) < limit\n");
}

#[test]
fn chained_cast_roundtrips() {
    let synth = Synth::new();
    let inner = synth.type_cast(synth.name_expr("x"), synth.ty_named("any"));
    let outer = synth.type_cast(inner, synth.ty_named("number"));
    let block = synth.block(vec![synth.local(&["narrowed"], vec![outer])], None);
    assert_format_block(&block, LuaVersion::Luau, "local narrowed = (x :: any) :: number\n");
}

#[test]
fn const_declarations_roundtrip() {
    let synth = Synth::new();
    let const_local = synth
        .const_local(vec![synth.attributed_name("frozen", None, None)], vec![synth.number_i64(1)]);
    let const_function = synth.const_function(
        "pinned",
        Vec::new(),
        FunctionSignature::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![const_local, const_function], None);
    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
const frozen = 1

const function pinned() end
",
    );
}

#[test]
fn interpolated_parts_roundtrip() {
    let synth = Synth::new();
    let greeting = synth.interpolated_string(vec![
        InterpolatedStringPart::Text("hi "),
        InterpolatedStringPart::Expr(synth.name_expr("name")),
        InterpolatedStringPart::Text("!"),
    ]);
    let plain = synth.interpolated_string(vec![InterpolatedStringPart::Text("no exprs")]);
    let block = synth.block(vec![synth.local(&["s", "t"], vec![greeting, plain])], None);
    assert_format_block(&block, LuaVersion::Luau, "local s, t = `hi {name}!`, `no exprs`\n");
}

#[test]
fn exponent_number_roundtrips() {
    let synth = Synth::new();
    let block = synth.block(vec![synth.local(&["huge"], vec![synth.number_f64(1e300)])], None);
    assert_format_block(&block, LuaVersion::Luau, "local huge = 1e300\n");
}

#[test]
fn luau_pinned_numbers_roundtrip() {
    // Luau has no float subtype to preserve, so integral values print as
    // plain digits and `i64::MIN` as a negated decimal.
    let synth = Synth::new().with_version(LuaVersion::Luau);
    let values = vec![synth.number_f64(100.0), synth.number_f64(1.5), synth.number_i64(i64::MIN)];
    let block = synth.block(vec![], Some(synth.return_(vec![synth.array(values)])));
    assert_format_block(&block, LuaVersion::Luau, "return { 100, 1.5, -9223372036854775808 }\n");
}

#[test]
fn single_value_truncation_roundtrips() {
    let synth = Synth::new();
    let truncated = synth.single_value(synth.call(synth.name_expr("f"), vec![]));
    let block = synth.block(vec![], Some(synth.return_(vec![truncated])));
    assert_format_block(&block, LuaVersion::Luau, "return (f())\n");
}

#[test]
fn long_string_carriage_return_roundtrips() {
    let synth = Synth::new();
    let text = synth.long_string("a\r\nb");
    let block = synth.block(vec![synth.local(&["doc"], vec![text])], None);
    assert_format_block(&block, LuaVersion::Luau, "local doc = \"a\\r\\nb\"\n");
}

#[test]
fn version_pinned_keyword_field_roundtrips() {
    let synth = Synth::new().with_version(LuaVersion::Luau);
    // `goto` is a plain identifier in Luau, so the dot form is used and
    // must survive a Luau re-parse.
    let access = synth.field_or_index(synth.name_expr("t"), "goto");
    let block = synth.block(vec![], Some(synth.return_(vec![access])));
    assert_format_block(&block, LuaVersion::Luau, "return t.goto\n");
}

#[test]
fn synthetic_comments_placed_around_stmt() {
    let synth = Synth::new();
    let stmt = synth.local(&["x"], vec![synth.number("1")]);
    let leading = SyntheticComment::leading(&stmt, "leading note");
    let trailing = SyntheticComment::trailing(&stmt, "trailing note");
    let block = synth.block(vec![stmt], None);
    assert_format_block_with(
        &block,
        Comments::synthetic(vec![leading, trailing]),
        LuaVersion::Luau,
        &FormatOptions::default(),
        "\
-- leading note
local x = 1 -- trailing note
",
    );
}

#[test]
fn dangling_comment_prints_inside_empty_function() {
    let synth = Synth::new();
    let body = synth.block(vec![], None);
    let dangling = SyntheticComment::dangling(&body, "unreachable");
    let stmt = synth.local_function("stub", &[], body);
    let block = synth.block(vec![stmt], None);
    assert_format_block_with(
        &block,
        Comments::synthetic(vec![dangling]),
        LuaVersion::Luau,
        &FormatOptions::default(),
        "\
local function stub()
\t-- unreachable
end
",
    );
}

#[test]
fn requested_blank_lines_separate_stmts() {
    let synth = Synth::new();
    let first = synth.local(&["a"], vec![synth.number("1")]);
    let second = synth.local(&["b"], vec![synth.number("2")]);
    let blank_anchor = second.span().start;
    let block = synth.block(vec![first, second], None);

    assert_format_block(
        &block,
        LuaVersion::Luau,
        "\
local a = 1
local b = 2
",
    );
    assert_format_block_with(
        &block,
        Comments::synthetic(vec![]).with_blank_before([blank_anchor]),
        LuaVersion::Luau,
        &FormatOptions::default(),
        "\
local a = 1

local b = 2
",
    );
}

#[test]
fn empty_block_formats_to_parseable_output() {
    let synth = Synth::new();
    let block = synth.block(vec![], None);
    assert_format_block(&block, LuaVersion::Luau, "\n");
}

#[test]
fn last_stmt_only_roundtrips() {
    let synth = Synth::new();
    let return_stmt = synth.return_(vec![synth.number("42")]);
    let block = synth.block(vec![], Some(return_stmt));
    assert_format_block(&block, LuaVersion::Luau, "return 42\n");
}

#[test]
fn synthetic_comment_on_a_collapsible_body_keeps_it_expanded() {
    let synth = Synth::new();
    let body_stmt = synth.local(&["x"], vec![synth.number("1")]);
    let trailing = SyntheticComment::trailing(&body_stmt, "why");
    let body = synth.block(vec![body_stmt], None);
    let if_stmt = synth.if_(synth.name_expr("a"), body, vec![], None);
    let block = synth.block(vec![if_stmt], None);
    let options = FormatOptions {
        collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
        ..FormatOptions::default()
    };
    assert_format_block_with(
        &block,
        Comments::synthetic(vec![trailing]),
        LuaVersion::Luau,
        &options,
        "\
if a then
\tlocal x = 1 -- why
end
",
    );
}

#[test]
fn multi_line_synthetic_comment_stays_a_comment() {
    let synth = Synth::new();
    let stmt = synth.local(&["x"], vec![synth.number("1")]);
    let comment = SyntheticComment::leading(
        &stmt,
        "\
first line
y = 2",
    );
    let block = synth.block(vec![stmt], None);
    assert_format_block_with(
        &block,
        Comments::synthetic(vec![comment]),
        LuaVersion::Lua54,
        &FormatOptions::default(),
        "\
-- first line
-- y = 2
local x = 1
",
    );
}

/// `return x`, with `x` under `wraps` unary operators.
fn nested_negations(wraps: usize) -> Block {
    let synth = Synth::new();
    let mut expr = synth.name_expr("x");
    for _ in 0..wraps {
        expr = synth.unary_op(UnOp::Not, expr);
    }
    synth.block(vec![], Some(synth.return_(vec![expr])))
}

#[test]
fn a_tree_at_the_nesting_cap_formats_and_reparses() {
    let wraps = MAX_NESTING_DEPTH - 1;
    let block = nested_negations(wraps);
    assert_eq!(block_nesting_depth(&block), MAX_NESTING_DEPTH, "the tree sits at the cap");
    assert_format_block(&block, LuaVersion::Lua54, &format!("return {}x\n", "not ".repeat(wraps)));
}

#[test]
fn a_tree_past_the_nesting_cap_is_an_error_instead_of_output_that_cannot_reparse() {
    let block = nested_negations(MAX_NESTING_DEPTH);
    let error = format_block(&block, Comments::none(), &FormatOptions::default())
        .expect_err("the tree nests one level too deep");
    assert!(error.message.contains("maximum nesting depth exceeded"), "{error:?}");
}

#[test]
fn a_tree_far_past_the_nesting_cap_is_refused_without_walking_down_it() {
    let synth = Synth::new();
    let mut nested = synth.name_expr("x");
    for _ in 0..200_000u32 {
        nested = synth.parenthesized(nested);
    }
    let block = synth.block(vec![], Some(synth.return_(vec![nested])));
    assert_eq!(block_nesting_depth(&block), MAX_NESTING_DEPTH + 1, "measuring stops there");
    assert_eq!(format_block(&block, Comments::none(), &FormatOptions::default()).ok(), None);

    // Dropped whole, a tree this deep recurses once per level.
    let Some(LastStatement::Return(mut return_stmt)) = block.last_stmt.map(|last| *last) else {
        panic!("the block ends in its return");
    };
    let mut expr = return_stmt.exprs.items.pop().expect("the return has its value");
    while let Expression::Parenthesized(mut parenthesized) = expr {
        expr = parenthesized.expr.take();
    }
}

#[test]
fn a_run_of_any_length_formats_as_one_level() {
    let synth = Synth::new();
    let (links, calls) = (50_000usize, 2_000usize);
    let mut sum = synth.name_expr("x");
    let mut methods = synth.name_expr("object");
    for _ in 0..links {
        sum = synth.binary_op(sum, BinOp::Add, synth.number("1"));
    }
    for _ in 0..calls {
        methods = synth.method_call(methods, "next", Vec::new());
    }
    let block = synth.block(vec![], Some(synth.return_(vec![sum, methods])));
    assert_eq!(block_nesting_depth(&block), 2);
    let expected = format!(
        "\
return x
{}\t\t+ 1,
\tobject
{}",
        "\t\t+ 1\n".repeat(links.saturating_sub(1)),
        "\t\t:next()\n".repeat(calls)
    );
    assert_format_block(&block, LuaVersion::Lua54, &expected);
}
