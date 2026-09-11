//! End-to-end coverage of the AST-in path (`format_block`). Each test formats
//! a programmatically built tree with no source text, re-parses the output,
//! and requires it to be error-free and structurally identical to the tree it
//! started from. Source-based tests cannot prove this, since they always have
//! an original text to lean on.

use luck_ast::Block;
use luck_ast::synth::{FnSig, Synth, SynthField, SynthInterpPart, SynthTypeField, TypeFieldAccess};
use luck_token::{BinOp, LuaVersion, UnOp};

use luck_formatter::{Comments, FormatOptions, blocks_equiv, format_block};

/// Format a synthetic block, re-parse the output under `version`, and assert
/// it re-parses cleanly and stays structurally equivalent.
fn assert_roundtrips_in(block: &Block, version: LuaVersion) -> String {
    let output = format_block(block, Comments::none(), &FormatOptions::default());
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "synthetic output failed to re-parse:\n{output}\nerrors: {:?}",
        reparsed.errors,
    );
    if let Err(diff) = blocks_equiv(block, &reparsed.block) {
        panic!("synthetic block not equivalent after round-trip:\n{output}\ndiff: {diff:?}");
    }
    output
}

/// Round-trip as Luau (the widest grammar).
fn assert_roundtrips(block: &Block) -> String {
    assert_roundtrips_in(block, LuaVersion::Luau)
}

#[test]
fn long_string_delimiters_and_bytes_roundtrip() {
    let synth = Synth::new();
    let mut contents = vec![
        String::new(),
        "]]=".to_string(),
        "a\0b".to_string(),
        "\n\0\r".to_string(),
    ];
    for level in 0..8 {
        let mut content = String::new();
        for preceding in 0..level {
            content.push_str(&format!("]{}]", "=".repeat(preceding)));
        }
        content.push_str(&format!("]{}", "=".repeat(level)));
        contents.push(content);
    }
    for content in contents {
        let block = synth.block(
            vec![],
            Some(synth.return_(vec![synth.long_string(&content)])),
        );
        for version in [
            LuaVersion::Lua51,
            LuaVersion::Lua52,
            LuaVersion::Lua53,
            LuaVersion::Lua54,
            LuaVersion::Lua55,
            LuaVersion::Luau,
        ] {
            let output = assert_roundtrips_in(&block, version);
            let lexed = luck_lexer::lex(&output, version);
            let literal = lexed
                .tokens
                .iter()
                .find_map(|token| match &token.kind {
                    luck_token::TokenKind::StringLiteral(text) => Some(text),
                    _ => None,
                })
                .expect("string literal");
            assert_eq!(
                luck_token::literal::decode_string_literal(literal, version).as_deref(),
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
            let block = synth.block(
                vec![synth.type_declaration(false, "Result", None, type_value)],
                None,
            );
            for width in [1, 60, 80, 120] {
                let options = FormatOptions {
                    line_width: width,
                    ..FormatOptions::default()
                };
                let output = format_block(&block, Comments::none(), &options);
                let parsed = luck_parser::parse(&output, LuaVersion::Luau);
                assert!(parsed.errors.is_empty(), "{output}: {:?}", parsed.errors);
                blocks_equiv(&block, &parsed.block).expect("type meaning preserved");
                let verified =
                    luck_formatter::format_and_verify(&output, LuaVersion::Luau, &options)
                        .expect("type formatting verifies");
                assert_eq!(verified.output, output);
            }
        }
    }
    let optional_function = synth.ty_optional(synth.ty_function(vec![], synth.ty_named("A")));
    let block = synth.block(
        vec![synth.type_declaration(false, "Callback", None, optional_function)],
        None,
    );
    assert_eq!(assert_roundtrips(&block), "type Callback = (() -> A)?\n");
}

#[test]
fn typed_local_roundtrips() {
    let synth = Synth::new();
    let optional = synth.ty_optional(synth.ty_named("number"));
    let stmt = synth.local_full(
        vec![synth.attributed_name("value", Some(optional), None)],
        vec![synth.nil()],
    );
    let block = synth.block(vec![stmt], None);

    let output = assert_roundtrips(&block);
    // Dropping the annotation is silent data loss, so it has to survive.
    assert!(
        output.contains("value: number?"),
        "type annotation dropped: {output}"
    );
}

#[test]
fn typed_function_roundtrips() {
    let synth = Synth::new();
    let sig = FnSig {
        params: vec![synth.param_typed("n", synth.ty_named("number"))],
        return_type: Some(synth.ty_named("number")),
        ..FnSig::default()
    };
    let ret = synth.return_(vec![synth.name_expr("n")]);
    let func = synth.function_def_full(Vec::new(), sig, synth.block(vec![], Some(ret)));
    let local = synth.local(&["identity"], vec![func]);
    let block = synth.block(vec![local], None);

    let output = assert_roundtrips(&block);
    assert!(
        output.contains("n: number"),
        "parameter annotation dropped: {output}"
    );
    assert!(
        output.contains("): number"),
        "return annotation dropped: {output}"
    );
}

#[test]
fn mixed_statements_roundtrip() {
    let synth = Synth::new();

    // local data = { 1, name = 2, ["k"] = true }
    let table = synth.table(vec![
        SynthField::Positional(synth.number("1")),
        SynthField::Named("name", synth.number("2")),
        SynthField::Bracketed(synth.string("k"), synth.boolean(true)),
    ]);
    let local_data = synth.local(&["data"], vec![table]);

    // for key, value in pairs(data) do end
    let iter = synth.call(synth.name_expr("pairs"), vec![synth.name_expr("data")]);
    let generic = synth.generic_for(
        vec![synth.param("key"), synth.param("value")],
        vec![iter],
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

    let block = synth.block(vec![local_data, generic, if_stmt], None);
    assert_roundtrips(&block);
}

#[test]
fn precedence_parens_roundtrip() {
    let synth = Synth::new();
    // (a + b) * c
    let grouped_sum = synth.binop(
        synth.binop(synth.name_expr("a"), BinOp::Add, synth.name_expr("b")),
        BinOp::Mul,
        synth.name_expr("c"),
    );
    // a - (b - c)
    let right_sub = synth.binop(
        synth.name_expr("a"),
        BinOp::Sub,
        synth.binop(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c")),
    );
    // (-a) ^ b
    let unary_power = synth.binop(
        synth.unop(UnOp::Neg, synth.name_expr("a")),
        BinOp::Pow,
        synth.name_expr("b"),
    );
    // (a .. b) .. c
    let left_concat = synth.binop(
        synth.binop(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b")),
        BinOp::Concat,
        synth.name_expr("c"),
    );
    let ret = synth.return_(vec![grouped_sum, right_sub, unary_power, left_concat]);
    let block = synth.block(vec![], Some(ret));

    let output = assert_roundtrips(&block);
    assert!(output.contains("(a + b) * c"), "grouping lost: {output}");
    assert!(output.contains("a - (b - c)"), "grouping lost: {output}");
    assert!(output.contains("(-a) ^ b"), "grouping lost: {output}");
    assert!(output.contains("(a .. b) .. c"), "grouping lost: {output}");
}

#[test]
fn if_expression_operand_roundtrips() {
    let synth = Synth::new();
    // a or (if c then 1 else 2): unparenthesized, the if-expression's else
    // branch would swallow everything after it.
    let if_expr = synth.if_expr(
        synth.name_expr("c"),
        synth.number("1"),
        vec![],
        synth.number("2"),
    );
    let guarded = synth.binop(synth.name_expr("a"), BinOp::Or, if_expr);
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
    let block = synth.block(
        vec![],
        Some(synth.return_(vec![string_method, table_field])),
    );
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
    assert!(
        output.contains("\\255") && output.contains("\\000"),
        "byte escapes missing: {output}"
    );
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
        synth.number_int(-42),
        synth.number_int(i64::MIN),
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
        vec![SynthField::Named("debug", synth.boolean(true))],
    );
    let block = synth.block(
        vec![synth.call_stmt(require), synth.call_stmt(configure)],
        None,
    );
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
    let decl = synth.global_decl(vec![synth.attributed_name("shared", None, None)], vec![]);
    let func = synth.global_function("main", FnSig::default(), synth.block(vec![], None));
    let star = synth.global_star(None);
    let block = synth.block(vec![decl, func, star], None);
    let output = assert_roundtrips_in(&block, LuaVersion::Lua55);
    assert!(output.contains("global shared"), "got: {output}");
    assert!(output.contains("global function main"), "got: {output}");
    assert!(output.contains("global *"), "got: {output}");
}

#[test]
fn mid_block_break_roundtrips_in_lua54() {
    let synth = Synth::new();
    let cond_break = synth.if_(
        synth.name_expr("done"),
        synth.block(vec![synth.break_stmt()], None),
        vec![],
        None,
    );
    let step = synth.call_stmt(synth.call(synth.name_expr("step"), vec![]));
    let loop_stmt = synth.while_(
        synth.boolean(true),
        synth.block(vec![cond_break, step], None),
    );
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
    let fn_generics = synth.generic_type_list(vec![("U", false)]);
    let fn_type = synth.ty_function_full(
        Some(fn_generics),
        vec![
            (Some("name"), synth.ty_named("string")),
            (None, synth.ty_generic_pack("T")),
        ],
        synth.ty_pack(vec![synth.ty_named("U"), synth.ty_named("boolean")]),
    );
    let alias = synth.type_declaration(true, "Handler", Some(generics), fn_type);

    // type Entry = { read id: number, [string]: boolean }
    let table_type = synth.ty_table(vec![
        SynthTypeField::Named {
            access: Some(TypeFieldAccess::Read),
            name: "id",
            value: synth.ty_named("number"),
        },
        SynthTypeField::Indexer {
            access: None,
            key: synth.ty_named("string"),
            value: synth.ty_named("boolean"),
        },
    ]);
    let entry = synth.type_declaration(false, "Entry", None, table_type);

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
    let cast = synth.type_cast(
        synth.name_expr("value"),
        synth.ty_typeof(synth.name_expr("template")),
    );
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
        FnSig::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![stmt], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("@native"), "attribute dropped: {output}");
}

#[test]
fn attribute_args_roundtrip() {
    let synth = Synth::new();
    let attribute = synth.function_attribute("deprecated", Some(vec![synth.string("use y")]));
    let stmt = synth.local_function_full(
        "old",
        vec![attribute],
        FnSig::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![stmt], None);
    let output = assert_roundtrips(&block);
    assert!(
        output.contains("@[deprecated(\"use y\")]"),
        "bracketed attribute lost: {output}"
    );
}

#[test]
fn cast_before_comparison_roundtrips() {
    let synth = Synth::new();
    // Bare `value :: number < limit` fails to parse: the type grammar reads
    // `<` as generic arguments. The synthesized form must carry parens.
    let cast = synth.type_cast(synth.name_expr("value"), synth.ty_named("number"));
    let compared = synth.binop(cast, BinOp::Lt, synth.name_expr("limit"));
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
    assert!(
        output.contains("(x :: any) :: number"),
        "chained cast not parenthesized: {output}"
    );
}

#[test]
fn const_declarations_roundtrip() {
    let synth = Synth::new();
    let const_local = synth.const_local(
        vec![synth.attributed_name("frozen", None, None)],
        vec![synth.number_int(1)],
    );
    let const_function = synth.const_function(
        "pinned",
        Vec::new(),
        FnSig::default(),
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
        SynthInterpPart::Text("hi "),
        SynthInterpPart::Expr(synth.name_expr("name")),
        SynthInterpPart::Text("!"),
    ]);
    let plain = synth.interpolated_string(vec![SynthInterpPart::Text("no exprs")]);
    let block = synth.block(vec![synth.local(&["s", "t"], vec![greeting, plain])], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("`hi {name}!`"), "got: {output}");
    assert!(output.contains("`no exprs`"), "got: {output}");
}

#[test]
fn exponent_number_roundtrips() {
    let synth = Synth::new();
    let block = synth.block(
        vec![synth.local(&["huge"], vec![synth.number_f64(1e300)])],
        None,
    );
    let output = assert_roundtrips(&block);
    assert!(output.contains("1e300"), "exponent form lost: {output}");
}

#[test]
fn luau_pinned_numbers_roundtrip() {
    // Pinned to Luau there is no float subtype to preserve, so integral
    // values print as plain digits and i64::MIN as a negated decimal.
    let synth = Synth::new().with_version(LuaVersion::Luau);
    let values = vec![
        synth.number_f64(100.0),
        synth.number_f64(1.5),
        synth.number_int(i64::MIN),
    ];
    let block = synth.block(vec![], Some(synth.return_(vec![synth.array(values)])));
    let output = assert_roundtrips(&block);
    assert!(output.contains("100,"), "plain integer form lost: {output}");
    assert!(
        !output.contains("1e2") && !output.contains("100.0"),
        "float-subtype marker leaked: {output}"
    );
    assert!(
        output.contains("-9223372036854775808"),
        "i64::MIN decimal form lost: {output}"
    );
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
fn synthetic_comments_placed_around_statement() {
    let synth = Synth::new();
    let stmt = synth.local(&["x"], vec![synth.number("1")]);
    let leading = synth.leading_comment(&stmt, "leading note");
    let trailing = synth.trailing_comment(&stmt, "trailing note");
    let block = synth.block(vec![stmt], None);

    let output = format_block(
        &block,
        Comments::synthetic(vec![leading, trailing]),
        &FormatOptions::default(),
    );

    assert!(
        output.contains("-- leading note"),
        "leading comment missing: {output}"
    );
    assert!(
        output.contains("-- trailing note"),
        "trailing comment missing: {output}"
    );

    let lines: Vec<&str> = output.lines().collect();
    let leading_line = lines
        .iter()
        .position(|line| line.contains("leading note"))
        .expect("leading comment line");
    let stmt_line = lines
        .iter()
        .position(|line| line.contains("local x = 1"))
        .expect("statement line");
    // Leading comment prints on its own line, before the statement.
    assert!(
        leading_line < stmt_line,
        "leading comment not before statement: {output}"
    );
    assert!(
        !lines[leading_line].contains("local x = 1"),
        "leading comment must be on its own line: {output}"
    );
    // Trailing comment prints as a suffix on the statement's line.
    assert!(
        lines[stmt_line].contains("-- trailing note"),
        "trailing comment not on statement line: {output}"
    );
}

#[test]
fn dangling_comment_prints_inside_empty_function() {
    let synth = Synth::new();
    let body = synth.block(vec![], None);
    let dangling = synth.dangling_comment(&body, "unreachable");
    let stmt = synth.local_function("stub", &[], body);
    let block = synth.block(vec![stmt], None);

    let output = format_block(
        &block,
        Comments::synthetic(vec![dangling]),
        &FormatOptions::default(),
    );
    let comment_line = output
        .lines()
        .position(|line| line.contains("-- unreachable"))
        .expect("dangling comment printed");
    let end_line = output
        .lines()
        .position(|line| line.trim() == "end")
        .expect("end line");
    assert!(
        comment_line < end_line,
        "comment must sit inside the body: {output}"
    );
}

#[test]
fn requested_blank_lines_separate_statements() {
    let synth = Synth::new();
    let first = synth.local(&["a"], vec![synth.number("1")]);
    let second = synth.local(&["b"], vec![synth.number("2")]);
    let blank_anchor = second.span().start;
    let block = synth.block(vec![first, second], None);

    let without = format_block(&block, Comments::none(), &FormatOptions::default());
    assert!(
        !without.contains("\n\n"),
        "no blank expected by default: {without:?}"
    );

    let with = format_block(
        &block,
        Comments::synthetic(vec![]).with_blank_before([blank_anchor]),
        &FormatOptions::default(),
    );
    assert!(
        with.contains("local a = 1\n\nlocal b = 2"),
        "requested blank missing: {with:?}"
    );
}

#[test]
fn empty_block_does_not_panic() {
    let synth = Synth::new();
    let block = synth.block(vec![], None);
    let output = format_block(&block, Comments::none(), &FormatOptions::default());
    let reparsed = luck_parser::parse(&output, LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "empty block produced unparseable output: {output:?}"
    );
}

#[test]
fn last_statement_only_roundtrips() {
    let synth = Synth::new();
    let ret = synth.return_(vec![synth.number("42")]);
    let block = synth.block(vec![], Some(ret));
    assert_roundtrips(&block);
}
