//! End-to-end proof of the AST-in path (`format_block`), the decompiler
//! contract: format a programmatically built tree with no source text, then
//! re-parse the output and require it be error-free and structurally identical
//! to the tree we started from. This is the guarantee source-based tests can't
//! give - there is no original text to lean on, only the AST.

use luck_ast::Block;
use luck_ast::synth::{Synth, SynthField};
use luck_token::LuaVersion;

use luck_formatter::{Comments, FormatOptions, blocks_equiv, format_block};

/// Format a synthetic block as Luau (the widest grammar), re-parse the output,
/// and assert it re-parses cleanly and stays structurally equivalent.
fn assert_roundtrips(block: &Block) -> String {
    let output = format_block(block, Comments::none(), &FormatOptions::default());
    let reparsed = luck_parser::parse(&output, LuaVersion::Luau);
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

#[test]
fn typed_local_roundtrips() {
    let mut synth = Synth::new();
    let number = synth.ty_named("number");
    let optional = synth.ty_optional(number);
    let nil = synth.nil();
    let stmt = synth.local_typed(vec![("value", Some(optional))], vec![nil]);
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
    let mut synth = Synth::new();
    let param_type = synth.ty_named("number");
    let param = synth.param_typed("n", param_type);
    let return_type = synth.ty_named("number");
    let name = synth.name_expr("n");
    let ret = synth.return_(vec![name]);
    let body = synth.block(vec![], Some(ret));
    let func = synth.function_def_typed(vec![param], Some(return_type), body);
    let local = synth.local(vec!["identity"], vec![func]);
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
    let mut synth = Synth::new();

    // local data = { 1, name = 2, ["k"] = true }
    let one = synth.number("1");
    let two = synth.number("2");
    let key = synth.string("k");
    let flag = synth.bool(true);
    let table = synth.table(vec![
        SynthField::Positional(one),
        SynthField::Named("name".to_string(), two),
        SynthField::Bracketed(key, flag),
    ]);
    let local_data = synth.local(vec!["data"], vec![table]);

    // for key, value in pairs(data) do end
    let pairs = synth.name_expr("pairs");
    let data_ref = synth.name_expr("data");
    let iter = synth.call(pairs, vec![data_ref]);
    let for_body = synth.block(vec![], None);
    let generic = synth.generic_for(vec!["key", "value"], vec![iter], for_body);

    // if data then data:insert(3) elseif false then else end
    let cond = synth.name_expr("data");
    let receiver = synth.name_expr("data");
    let arg = synth.number("3");
    let method = synth.method_call(receiver, "insert", vec![arg]);
    let method_stmt = synth.call_stmt(method);
    let then_block = synth.block(vec![method_stmt], None);
    let elseif_cond = synth.bool(false);
    let elseif_block = synth.block(vec![], None);
    let else_block = synth.block(vec![], None);
    let if_stmt = synth.if_(
        cond,
        then_block,
        vec![(elseif_cond, elseif_block)],
        Some(else_block),
    );

    let block = synth.block(vec![local_data, generic, if_stmt], None);
    assert_roundtrips(&block);
}

#[test]
fn synthetic_comments_placed_around_statement() {
    let mut synth = Synth::new();
    let one = synth.number("1");
    let stmt = synth.local(vec!["x"], vec![one]);
    // Comments anchor on the target statement's span start.
    let anchor = stmt.span().start;
    let leading = synth.comment(anchor, "leading note", true);
    let trailing = synth.comment(anchor, "trailing note", false);
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
fn empty_block_does_not_panic() {
    let mut synth = Synth::new();
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
    let mut synth = Synth::new();
    let value = synth.number("42");
    let ret = synth.return_(vec![value]);
    let block = synth.block(vec![], Some(ret));
    assert_roundtrips(&block);
}
