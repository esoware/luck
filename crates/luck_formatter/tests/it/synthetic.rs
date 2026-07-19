//! End-to-end proof of the AST-in path (`format_block`), the decompiler
//! contract: format a programmatically built tree with no source text, then
//! re-parse the output and require it be error-free and structurally identical
//! to the tree we started from. This is the guarantee source-based tests can't
//! give - there is no original text to lean on, only the AST.

use luck_ast::Block;
use luck_ast::synth::{FnSig, Synth, SynthField, SynthTypeField, TypeFieldAccess};
use luck_token::{LuaVersion, TokenKind};

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
fn typed_local_roundtrips() {
    let synth = Synth::new();
    let optional = synth.ty_optional(synth.ty_named("number"));
    let stmt = synth.local_full(
        vec![synth.attributed_name("value", Some(optional), None)],
        vec![synth.nil()],
    );
    let block = synth.block(vec![stmt], None);

    let output = assert_roundtrips(&block);
    // The annotation must survive; losing it is the data-loss bug the rewrite
    // set out to fix.
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
    let func = synth.function_def_full(sig, synth.block(vec![], Some(ret)));
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
        synth.binop(synth.name_expr("a"), TokenKind::Plus, synth.name_expr("b")),
        TokenKind::Star,
        synth.name_expr("c"),
    );
    // a - (b - c)
    let right_sub = synth.binop(
        synth.name_expr("a"),
        TokenKind::Minus,
        synth.binop(synth.name_expr("b"), TokenKind::Minus, synth.name_expr("c")),
    );
    // (-a) ^ b
    let unary_power = synth.binop(
        synth.unop(TokenKind::Minus, synth.name_expr("a")),
        TokenKind::Caret,
        synth.name_expr("b"),
    );
    // (a .. b) .. c
    let left_concat = synth.binop(
        synth.binop(
            synth.name_expr("a"),
            TokenKind::DotDot,
            synth.name_expr("b"),
        ),
        TokenKind::DotDot,
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
    let guarded = synth.binop(synth.name_expr("a"), TokenKind::Or, if_expr);
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
    let decl = synth.global_decl(vec![synth.attributed_name("shared", None, None)]);
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
        &["native"],
        FnSig::default(),
        synth.block(vec![], None),
    );
    let block = synth.block(vec![stmt], None);
    let output = assert_roundtrips(&block);
    assert!(output.contains("@native"), "attribute dropped: {output}");
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
