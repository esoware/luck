//! Most tests format a tree built in code with no source text, then require
//! the output to re-parse cleanly into a tree equivalent to the one they
//! started from.
//!
//! Source-based tests can't show this, since they always have
//! the original text to lean on.

use luck_ast::{
    Block, FieldSpec, FunctionSignature, InterpolatedStringPart, Synth, SyntheticComment,
    TypeFieldAccess, TypeFieldSpec,
};
use luck_formatter::{
    CollapseSimpleStatement, Comments, FormatOptions, blocks_equivalent, format_block,
};
use luck_token::{BinOp, LuaVersion, TokenKind, UnOp, decode_string};

/// Format a synthetic block and assert the output re-parses cleanly under
/// `version` into an equivalent tree. Returns the output.
fn assert_roundtrips_in(block: &Block, version: LuaVersion) -> String {
    let output = format_block(block, Comments::none(), &FormatOptions::default());
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "synthetic output failed to re-parse:\n{output}\nerrors: {:?}",
        reparsed.errors,
    );
    let equivalence = blocks_equivalent(block, &reparsed.block, version);
    assert!(
        equivalence.is_ok(),
        "synthetic block not equivalent after round-trip:\n{output}\ndiff: {equivalence:?}"
    );
    output
}

fn assert_roundtrips(block: &Block) -> String {
    assert_roundtrips_in(block, LuaVersion::Luau)
}

#[test]
fn long_string_delimiters_and_bytes_roundtrip() {
    let synth = Synth::new();
    let mut contents =
        vec![String::new(), "]]=".to_owned(), "a\0b".to_owned(), "\n\0\r".to_owned()];
    for level in 0..8 {
        let mut content = String::new();
        for preceding in 0..level {
            content.push(']');
            content.push_str(&"=".repeat(preceding));
            content.push(']');
        }
        content.push(']');
        content.push_str(&"=".repeat(level));
        contents.push(content);
    }
    for content in contents {
        let block = synth.block(vec![], Some(synth.return_(vec![synth.long_string(&content)])));
        for version in LuaVersion::ALL {
            let output = assert_roundtrips_in(&block, version);
            let lexed = luck_lexer::lex(&output, version);
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
                "{version:?}: {output}"
            );
            assert!(!output.contains('\0'));
        }
    }
}

#[test]
fn composite_types_preserve_grouping() {
    let synth = Synth::new();
    let children = [
        synth.ty_union(vec![synth.ty_named("A"), synth.ty_named("B")]),
        synth.ty_intersection(vec![synth.ty_named("A"), synth.ty_named("B")]),
        synth.ty_function(vec![], synth.ty_named("A")),
        synth.ty_optional(synth.ty_named("A")),
    ];
    for child in children {
        let types = [
            synth.ty_optional(child.clone()),
            synth.ty_union(vec![child.clone(), synth.ty_named("C")]),
            synth.ty_union(vec![synth.ty_named("C"), child.clone()]),
            synth.ty_intersection(vec![child.clone(), synth.ty_named("C")]),
            synth.ty_intersection(vec![synth.ty_named("C"), child]),
        ];
        for type_value in types {
            let block =
                synth.block(vec![synth.type_declaration(false, "Result", None, type_value)], None);
            for width in [1, 60, 80, 120] {
                let options = FormatOptions { line_width: width, ..FormatOptions::default() };
                let output = format_block(&block, Comments::none(), &options);
                let parsed = luck_parser::parse(&output, LuaVersion::Luau);
                assert!(parsed.errors.is_empty(), "{output}: {:?}", parsed.errors);
                blocks_equivalent(&block, &parsed.block, LuaVersion::Luau)
                    .expect("type meaning preserved");
                let verified =
                    luck_formatter::format_and_verify(&output, LuaVersion::Luau, &options)
                        .expect("type formatting verifies");
                assert_eq!(verified.output, output);
            }
        }
    }
    let optional_function = synth.ty_optional(synth.ty_function(vec![], synth.ty_named("A")));
    let block =
        synth.block(vec![synth.type_declaration(false, "Callback", None, optional_function)], None);
    assert_eq!(assert_roundtrips(&block), "type Callback = (() -> A)?\n");
}

#[test]
fn typed_local_roundtrips() {
    let synth = Synth::new();
    let optional = synth.ty_optional(synth.ty_named("number"));
    let stmt = synth
        .local_full(vec![synth.attributed_name("value", Some(optional), None)], vec![synth.nil()]);
    let block = synth.block(vec![stmt], None);

    let output = assert_roundtrips(&block);
    assert!(output.contains("value: number?"), "type annotation dropped: {output}");
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

    let output = assert_roundtrips(&block);
    assert!(output.contains("n: number"), "parameter annotation dropped: {output}");
    assert!(output.contains("): number"), "return annotation dropped: {output}");
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
    assert_roundtrips(&block);
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

    let output = assert_roundtrips(&block);
    assert!(output.contains("(a + b) * c"), "grouping lost: {output}");
    assert!(output.contains("a - (b - c)"), "grouping lost: {output}");
    assert!(output.contains("(-a) ^ b"), "grouping lost: {output}");
    assert!(output.contains("(a .. b) .. c"), "grouping lost: {output}");
}

#[test]
fn if_expr_operand_roundtrips() {
    let synth = Synth::new();
    // a or (if c then 1 else 2): unparenthesized, the if-expression's else
    // branch would swallow everything after it.
    let if_expr = synth.if_expr(synth.name_expr("c"), synth.number("1"), vec![], synth.number("2"));
    let guarded = synth.binary_op(synth.name_expr("a"), BinOp::Or, if_expr);
    let block = synth.block(vec![], Some(synth.return_(vec![guarded])));
    let output = assert_roundtrips(&block);
    assert!(
        output.contains("(if c then 1 else 2)"),
        "if-expression operand not parenthesized: {output}"
    );
}

#[test]
fn prefix_wrapping_roundtrips() {
    let synth = Synth::new();
    // ("s"):rep(2) and ({}).field both need their receivers parenthesized.
    let string_method = synth.method_call(synth.string("s"), "rep", vec![synth.number("2")]);
    let table_field = synth.field(synth.table(vec![]), "field");
    let block = synth.block(vec![], Some(synth.return_(vec![string_method, table_field])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("(\"s\"):rep(2)"), "got: {output}");
    assert!(output.contains("({}).field"), "got: {output}");
}

#[test]
fn field_or_index_roundtrips() {
    let synth = Synth::new();
    let good = synth.field_or_index(synth.name_expr("t"), "ok");
    let bad = synth.field_or_index(synth.name_expr("t"), "not ok");
    let keyword = synth.field_or_index(synth.name_expr("t"), "end");
    let block = synth.block(vec![], Some(synth.return_(vec![good, bad, keyword])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("t.ok"), "got: {output}");
    assert!(output.contains("t[\"not ok\"]"), "got: {output}");
    assert!(output.contains("t[\"end\"]"), "got: {output}");
}

#[test]
fn string_bytes_roundtrips() {
    let synth = Synth::new();
    let bytes = synth.string_bytes(&[0xff, 0x00, b'a', b'1']);
    let block = synth.block(vec![synth.local(&["blob"], vec![bytes])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("\\255") && output.contains("\\0a"), "byte escapes missing: {output}");
}

#[test]
fn long_string_roundtrips() {
    let synth = Synth::new();
    let text = synth.long_string("line one\nline ]] two");
    let block = synth.block(vec![synth.local(&["doc"], vec![text])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("[=[line one"), "got: {output}");
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
    let output = assert_roundtrips(&block);
    assert!(output.contains("3.0"), "float subtype lost: {output}");
    assert!(output.contains("1 / 0"), "infinity form lost: {output}");
    assert!(output.contains("0x8000000000000000"), "got: {output}");
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
    assert_roundtrips(&block);
}

#[test]
fn attributed_local_roundtrips_in_lua54() {
    let synth = Synth::new();
    let stmt = synth.local_full(
        vec![synth.attributed_name("frozen", None, Some("const"))],
        vec![synth.number("1")],
    );
    let block = synth.block(vec![stmt], None);
    let output = assert_roundtrips_in(&block, LuaVersion::Lua54);
    assert!(output.contains("frozen <const>"), "got: {output}");
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
    let output = assert_roundtrips_in(&block, LuaVersion::Lua55);
    assert!(output.contains("global shared"), "got: {output}");
    assert!(output.contains("global function main"), "got: {output}");
    assert!(output.contains("global *"), "got: {output}");
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
    assert_roundtrips_in(&block, LuaVersion::Lua54);
}

#[test]
fn goto_roundtrips_in_lua54() {
    let synth = Synth::new();
    let block = synth.block(vec![synth.goto_("done"), synth.label("done")], None);
    assert_roundtrips_in(&block, LuaVersion::Lua54);
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
    let output = assert_roundtrips(&block);
    assert!(output.contains("export type Handler"), "got: {output}");
    assert!(output.contains("read id: number"), "got: {output}");
    assert!(output.contains("typeof(template)"), "got: {output}");
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
    let output = assert_roundtrips(&block);
    assert!(output.contains("@native"), "attribute dropped: {output}");
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
    let output = assert_roundtrips(&block);
    assert!(
        output.contains("@[deprecated({ use = \"y\" })]"),
        "bracketed attribute lost: {output}"
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
    let output = assert_roundtrips(&block);
    assert!(
        output.contains("(value :: number) < limit"),
        "cast not parenthesized before <: {output}"
    );
}

#[test]
fn chained_cast_roundtrips() {
    let synth = Synth::new();
    let inner = synth.type_cast(synth.name_expr("x"), synth.ty_named("any"));
    let outer = synth.type_cast(inner, synth.ty_named("number"));
    let block = synth.block(vec![synth.local(&["narrowed"], vec![outer])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("(x :: any) :: number"), "chained cast not parenthesized: {output}");
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
    let output = assert_roundtrips(&block);
    assert!(output.contains("const frozen = 1"), "got: {output}");
    assert!(output.contains("const function pinned"), "got: {output}");
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
    let output = assert_roundtrips(&block);
    assert!(output.contains("`hi {name}!`"), "got: {output}");
    assert!(output.contains("`no exprs`"), "got: {output}");
}

#[test]
fn exponent_number_roundtrips() {
    let synth = Synth::new();
    let block = synth.block(vec![synth.local(&["huge"], vec![synth.number_f64(1e300)])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("1e300"), "exponent form lost: {output}");
}

#[test]
fn luau_pinned_numbers_roundtrip() {
    // Luau has no float subtype to preserve, so integral values print as
    // plain digits and `i64::MIN` as a negated decimal.
    let synth = Synth::new().with_version(LuaVersion::Luau);
    let values = vec![synth.number_f64(100.0), synth.number_f64(1.5), synth.number_i64(i64::MIN)];
    let block = synth.block(vec![], Some(synth.return_(vec![synth.array(values)])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("100,"), "plain integer form lost: {output}");
    assert!(
        !output.contains("1e2") && !output.contains("100.0"),
        "float-subtype marker leaked: {output}"
    );
    assert!(output.contains("-9223372036854775808"), "i64::MIN decimal form lost: {output}");
}

#[test]
fn single_value_truncation_roundtrips() {
    let synth = Synth::new();
    let truncated = synth.single_value(synth.call(synth.name_expr("f"), vec![]));
    let block = synth.block(vec![], Some(synth.return_(vec![truncated])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("return (f())"), "got: {output}");
}

#[test]
fn long_string_carriage_return_roundtrips() {
    let synth = Synth::new();
    let text = synth.long_string("a\r\nb");
    let block = synth.block(vec![synth.local(&["doc"], vec![text])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("\"a\\r\\nb\""), "got: {output}");
}

#[test]
fn version_pinned_keyword_field_roundtrips() {
    let synth = Synth::new().with_version(LuaVersion::Luau);
    // `goto` is a plain identifier in Luau, so the dot form is used and
    // must survive a Luau re-parse.
    let access = synth.field_or_index(synth.name_expr("t"), "goto");
    let block = synth.block(vec![], Some(synth.return_(vec![access])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("t.goto"), "got: {output}");
}

#[test]
fn synthetic_comments_placed_around_stmt() {
    let synth = Synth::new();
    let stmt = synth.local(&["x"], vec![synth.number("1")]);
    let leading = SyntheticComment::leading(&stmt, "leading note");
    let trailing = SyntheticComment::trailing(&stmt, "trailing note");
    let block = synth.block(vec![stmt], None);

    let output = format_block(
        &block,
        Comments::synthetic(vec![leading, trailing]),
        &FormatOptions::default(),
    );

    assert!(output.contains("-- leading note"), "leading comment missing: {output}");
    assert!(output.contains("-- trailing note"), "trailing comment missing: {output}");

    let lines: Vec<&str> = output.lines().collect();
    let leading_line =
        lines.iter().position(|line| line.contains("leading note")).expect("leading comment line");
    let stmt_line =
        lines.iter().position(|line| line.contains("local x = 1")).expect("statement line");
    assert!(leading_line < stmt_line, "leading comment not before statement: {output}");
    assert!(
        !lines[leading_line].contains("local x = 1"),
        "leading comment must be on its own line: {output}"
    );
    assert!(
        lines[stmt_line].contains("-- trailing note"),
        "trailing comment not on statement line: {output}"
    );
}

#[test]
fn dangling_comment_prints_inside_empty_function() {
    let synth = Synth::new();
    let body = synth.block(vec![], None);
    let dangling = SyntheticComment::dangling(&body, "unreachable");
    let stmt = synth.local_function("stub", &[], body);
    let block = synth.block(vec![stmt], None);

    let output =
        format_block(&block, Comments::synthetic(vec![dangling]), &FormatOptions::default());
    let comment_line = output
        .lines()
        .position(|line| line.contains("-- unreachable"))
        .expect("dangling comment printed");
    let end_line = output.lines().position(|line| line.trim() == "end").expect("end line");
    assert!(comment_line < end_line, "comment must sit inside the body: {output}");
}

#[test]
fn requested_blank_lines_separate_stmts() {
    let synth = Synth::new();
    let first = synth.local(&["a"], vec![synth.number("1")]);
    let second = synth.local(&["b"], vec![synth.number("2")]);
    let blank_anchor = second.span().start;
    let block = synth.block(vec![first, second], None);

    let plain_output = format_block(&block, Comments::none(), &FormatOptions::default());
    assert!(!plain_output.contains("\n\n"), "no blank expected by default: {plain_output:?}");

    let spaced_output = format_block(
        &block,
        Comments::synthetic(vec![]).with_blank_before([blank_anchor]),
        &FormatOptions::default(),
    );
    assert!(
        spaced_output.contains("local a = 1\n\nlocal b = 2"),
        "requested blank missing: {spaced_output:?}"
    );
}

#[test]
fn empty_block_formats_to_parseable_output() {
    let synth = Synth::new();
    let block = synth.block(vec![], None);
    let output = format_block(&block, Comments::none(), &FormatOptions::default());
    let reparsed = luck_parser::parse(&output, LuaVersion::Luau);
    assert!(reparsed.errors.is_empty(), "empty block produced unparseable output: {output:?}");
}

#[test]
fn last_stmt_only_roundtrips() {
    let synth = Synth::new();
    let return_stmt = synth.return_(vec![synth.number("42")]);
    let block = synth.block(vec![], Some(return_stmt));
    assert_roundtrips(&block);
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

    let output = format_block(&block, Comments::synthetic(vec![trailing]), &options);

    assert_eq!(output, "if a then\n\tlocal x = 1 -- why\nend\n");
}

#[test]
fn multi_line_synthetic_comment_stays_a_comment() {
    let synth = Synth::new();
    let stmt = synth.local(&["x"], vec![synth.number("1")]);
    let comment = SyntheticComment::leading(&stmt, "first line\ny = 2");
    let block = synth.block(vec![stmt], None);

    let output =
        format_block(&block, Comments::synthetic(vec![comment]), &FormatOptions::default());

    let reparsed = luck_parser::parse(&output, LuaVersion::Lua54);
    assert_eq!(
        reparsed.block.stmts.len(),
        1,
        "every line of a comment must stay commented out: {output:?}"
    );
}
