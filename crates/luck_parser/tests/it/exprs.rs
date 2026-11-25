use luck_allocator::Allocator;
use luck_ast::{
    Expression, Field, FunctionArgs, GetSpan as _, InterpolatedSegment, Statement, Suffix,
};
use luck_parser::parse;
use luck_token::{BinOp, Span, UnOp};

use crate::common::{
    assert_has_errors, assert_no_errors, chain, chain_shape, local_value, parse_str, run_operands,
    run_ops,
};

#[test]
fn call_takes_a_string_argument() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "require \"foo\"");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert!(
        matches!(call.suffixes.last(), Suffix::Call(args) if matches!(args, FunctionArgs::StringLiteral(_)))
    );
}

#[test]
fn call_takes_a_table_argument() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "f{1, 2, 3}");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert!(
        matches!(call.suffixes.last(), Suffix::Call(args) if matches!(args, FunctionArgs::TableConstructor(_)))
    );
}

#[test]
fn nil_is_a_nil_literal() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = nil");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Nil(_)));
}

#[test]
fn true_and_false_are_boolean_literals() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local a, b = true, false");
    assert_no_errors(&result);
    if let Statement::LocalAssignment(local) = &result.chunk.stmts[0] {
        let exprs = &local.exprs;
        let first = &exprs[0];
        assert!(matches!(first, Expression::True(_)));
        let second = exprs.last().expect("expression list has last element");
        assert!(matches!(*second, Expression::False(_)));
    } else {
        panic!("expected LocalAssignment");
    }
}

#[test]
fn float_numeral_is_a_number_literal() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = 3.14");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::Number(_)));
}

#[test]
fn quoted_string_is_a_string_literal() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = \"hello world\"");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn mul_binds_tighter_than_add() {
    let allocator = Allocator::new();
    // `1 + 2 * 3` should parse as `1 + (2 * 3)`.
    let result = parse_str(&allocator, "local x = 1 + 2 * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Add]);
    assert_eq!(run_ops(run_operands(expr)[1]), [BinOp::Mul]);
}

#[test]
fn power_chain_is_one_run() {
    let allocator = Allocator::new();
    // `2 ^ 3 ^ 4` is one run, which groups from the right: `2 ^ (3 ^ 4)`.
    let result = parse_str(&allocator, "local x = 2 ^ 3 ^ 4");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Pow, BinOp::Pow]);
    assert!(run_operands(expr).iter().all(|operand| matches!(operand, Expression::Number(_))));
}

#[test]
fn concat_chain_is_one_run() {
    let allocator = Allocator::new();
    // `a .. b .. c` is one run, which groups from the right: `a .. (b .. c)`.
    let result = parse_str(&allocator, "local x = a .. b .. c");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::Concat, BinOp::Concat]);
}

#[test]
fn power_binds_tighter_than_unary_minus() {
    let allocator = Allocator::new();
    // `-a ^ b` should parse as `-(a ^ b)`, not `(-a) ^ b`.
    let result = parse_str(&allocator, "local x = -a ^ b");
    assert_no_errors(&result);
    let expr = local_value(&result);
    let Expression::UnaryOp(unary) = expr else {
        panic!("expected UnaryOp at top level, got {expr:?}");
    };
    assert_eq!(unary.op, UnOp::Neg, "outer should be unary minus");
    assert_eq!(run_ops(&unary.operand), [BinOp::Pow]);
}

#[test]
fn comparison_binds_tighter_than_and() {
    let allocator = Allocator::new();
    // `a < b and c > d` should parse as `(a < b) and (c > d)`.
    let result = parse_str(&allocator, "local x = a < b and c > d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::And]);
    let operands = run_operands(expr);
    assert_eq!(run_ops(operands[0]), [BinOp::Lt]);
    assert_eq!(run_ops(operands[1]), [BinOp::Gt]);
}

#[test]
fn concat_binds_tighter_than_comparison() {
    let allocator = Allocator::new();
    // `a .. b == c .. d` should parse as `(a .. b) == (c .. d)`.
    let result = parse_str(&allocator, "local x = a .. b == c .. d");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Eq]);
    let operands = run_operands(expr);
    assert_eq!(run_ops(operands[0]), [BinOp::Concat]);
    assert_eq!(run_ops(operands[1]), [BinOp::Concat]);
}

#[test]
fn not_is_a_unary_operator() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = not true");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::UnaryOp(_)));
}

#[test]
fn minus_before_a_numeral_is_a_unary_operator() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = -42");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::UnaryOp(_)));
}

#[test]
fn hash_is_the_length_operator() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = #t");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::UnaryOp(unary) = expr {
        assert!(matches!(unary.op, UnOp::Len));
    } else {
        panic!("expected UnaryOp, got {expr:?}");
    }
}

#[test]
fn empty_table_has_no_fields() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = {}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert!(table.fields.is_empty());
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn table_leading_separator_is_rejected() {
    let allocator = Allocator::new();
    // Luau rejects a leading separator: `{;}`, `{,}`, `{, 1}`.
    for source in ["local x = {;}", "local x = {,}", "local x = {, 1}"] {
        let result = parse_str(&allocator, source);
        assert!(!result.errors.is_empty(), "expected error for {source:?}, got none");
        // Recovery still produces a table constructor.
        assert!(matches!(local_value(&result), Expression::TableConstructor(_)));
    }
}

#[test]
fn table_keeps_positional_named_and_bracketed_fields() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = {1, 2, x = 3, [4] = 5}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert_eq!(table.fields.len(), 4);
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn table_takes_a_trailing_separator() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = {1,}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(table) = expr {
        assert_eq!(table.fields.len(), 1);
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn fields_chain_onto_one_prefix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = a.b.c");
    assert_no_errors(&result);
    assert_eq!(chain_shape(chain(local_value(&result))), "a.b.c");
}

#[test]
fn indexes_chain_onto_one_prefix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = a[1][2]");
    assert_no_errors(&result);
    assert_eq!(chain_shape(chain(local_value(&result))), "a[][]");
}

#[test]
fn method_calls_with_arguments_chain_onto_one_prefix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a:b(1):c(2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "a:b():c()");
}

#[test]
fn calls_chain_onto_one_prefix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "f(1)(2)");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "f()()");
}

#[test]
fn parentheses_group_without_a_node_of_their_own() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = (1 + 2) * 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Mul]);
    let sum = run_operands(expr)[0];
    assert_eq!(run_ops(sum), [BinOp::Add], "parentheses group without a node of their own");
    assert_eq!(result.groupings.around(sum.span()), Some(Span::new(10, 17)));
    assert_eq!(expr.span(), Span::new(10, 21), "a node spans the parentheses it starts with");
}

#[test]
fn parentheses_change_only_what_a_call_or_vararg_yields() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local function f(...) return (f()), (...), ((x)) end");
    assert_no_errors(&result);
    let Some(Statement::LocalFunction(function)) = result.chunk.stmts.first() else {
        panic!("expected a local function, got {:?}", result.chunk.stmts);
    };
    let Some(Statement::Return(return_stmt)) = function.body.block.stmts.last() else {
        panic!("expected a return");
    };
    assert!(matches!(
        *return_stmt.exprs,
        [Expression::Parenthesized(_), Expression::Parenthesized(_), Expression::Name(_)]
    ));
}

#[test]
fn and_binds_tighter_than_or() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = a and b or c");
    assert_no_errors(&result);
    // `and` binds tighter than `or`, so this is `(a and b) or c`.
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::Or]);
    assert_eq!(run_ops(run_operands(expr)[0]), [BinOp::And]);
}

#[test]
fn each_comparison_is_its_operator() {
    let allocator = Allocator::new();
    for (source, expected_op) in [
        ("local x = a < b", BinOp::Lt),
        ("local x = a >= b", BinOp::Ge),
        ("local x = a ~= b", BinOp::Ne),
        ("local x = a == b", BinOp::Eq),
    ] {
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
        assert_eq!(run_ops(local_value(&result)), [expected_op], "failed for: {source}");
    }
}

#[test]
fn long_string_ends_only_at_its_own_level() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = [===[hello ]]] ]==] world]===]");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(*expr, Expression::StringLiteral(_)));
}

#[test]
fn nested_tables_nest_in_the_tree() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = {{{{}}}}");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::TableConstructor(outer) = expr {
        assert_eq!(outer.fields.len(), 1);
        if let Field::Positional(Expression::TableConstructor(middle)) = &outer.fields[0] {
            assert_eq!(middle.fields.len(), 1);
            if let Field::Positional(Expression::TableConstructor(inner)) = &middle.fields[0] {
                assert_eq!(inner.fields.len(), 1);
                assert!(matches!(
                    &inner.fields[0],
                    Field::Positional(Expression::TableConstructor(innermost)) if innermost.fields.is_empty()
                ));
            } else {
                panic!("expected nested TableConstructor at depth 3");
            }
        } else {
            panic!("expected nested TableConstructor at depth 2");
        }
    } else {
        panic!("expected TableConstructor");
    }
}

#[test]
fn method_calls_without_arguments_chain_onto_one_prefix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "a:b():c():d()");
    assert_no_errors(&result);
    let Statement::FunctionCall(call) = &result.chunk.stmts[0] else {
        panic!("expected FunctionCall");
    };
    assert_eq!(chain_shape(call), "a:b():c():d()");
}

#[test]
fn parentheses_around_a_callee_read_on_unless_they_truncate() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "(f)(x); ((g).y).z(w); (h())()");
    assert_no_errors(&result);
    let [
        Statement::FunctionCall(call),
        Statement::FunctionCall(joined),
        Statement::FunctionCall(truncated),
    ] = &*result.chunk.stmts
    else {
        panic!("expected three calls, got {:?}", result.chunk.stmts);
    };
    assert_eq!(chain_shape(call), "f()");
    assert_eq!(chain_shape(joined), "g.y.z()", "a parenthesized chain reads on");
    assert_eq!(joined.span, Span::new(8, 20));
    assert_eq!(chain_shape(truncated), "()()", "a parenthesized call takes one value");
}

#[test]
fn a_call_spans_the_parentheses_its_callee_is_in() {
    let allocator = Allocator::new();
    let source = "(f)(x); (\"s\"):rep(2); (a).b.c()";
    let result = parse_str(&allocator, source);
    assert_no_errors(&result);
    let calls: Vec<&str> = result
        .chunk
        .stmts
        .iter()
        .filter_map(|stmt| {
            if let Statement::FunctionCall(chain) = stmt { chain.last_call() } else { None }
        })
        .map(|call| str::from_utf8(call.span().text(source.as_bytes())).unwrap())
        .collect();
    assert_eq!(calls, ["(f)(x)", "(\"s\"):rep(2)", "(a).b.c()"]);
    let Some(Statement::FunctionCall(fields)) = result.chunk.stmts.last() else {
        panic!("expected a call last");
    };
    assert_eq!(fields.head(1).span().text(source.as_bytes()), b"(a).b");
    assert_eq!(fields.head(0).span().text(source.as_bytes()), b"a", "a prefix alone is itself");
}

#[test]
fn double_slash_is_floor_division() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = a // b");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::FloorDiv]);
}

#[test]
fn if_expression_without_elseif_has_no_clauses() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = if a then 1 else 2");
    assert_no_errors(&result);
    assert_eq!(result.chunk.stmts.len(), 1);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::IfExpression(_)));
    if let Expression::IfExpression(if_expr) = expr {
        assert!(if_expr.elseif_clauses.is_empty(), "{:?}", if_expr.elseif_clauses);
    }
}

#[test]
fn if_expression_keeps_its_elseif_clause() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local y = if a then b elseif c then d else e");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::IfExpression(if_expr) = expr {
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    } else {
        panic!("expected IfExpression");
    }
}

#[test]
fn if_expression_nests_in_a_branch() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local z = if a then if b then 1 else 2 else 3");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::IfExpression(if_expr) = expr {
        assert!(matches!(&if_expr.then_expr, Expression::IfExpression(_)));
    } else {
        panic!("expected IfExpression");
    }
}

#[test]
fn backtick_string_is_an_interpolated_string() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = `hello`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::InterpolatedString(_)));
}

#[test]
fn interpolated_string_keeps_its_expression() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = `{y}`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        assert_eq!(interpolated_string.segments.len(), 1);
        assert!(matches!(interpolated_string.segments[0].expr, Expression::Name(_)));
        assert!(interpolated_string.tail.value.is_empty());
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn interpolated_string_keeps_each_segment_and_its_tail() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = `a{1+2}b{3}c`");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::InterpolatedString(interpolated_string) = expr {
        let texts: Vec<&[u8]> = interpolated_string
            .segments
            .iter()
            .map(|segment| segment.text.value.as_bytes())
            .collect();
        assert_eq!(texts, [b"a", b"b"]);
        assert!(matches!(
            &*interpolated_string.segments,
            [
                InterpolatedSegment { expr: Expression::BinaryOp(_), .. },
                InterpolatedSegment { expr: Expression::Number(_), .. },
            ]
        ));
        assert_eq!(interpolated_string.tail.value, &b"c"[..]);
    } else {
        panic!("expected InterpolatedString");
    }
}

#[test]
fn double_colon_is_a_type_cast() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = y :: number");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert!(matches!(expr, Expression::TypeCast(_)));
}

/// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so literals, tables,
/// and call results take a cast too. The minifier emits forms like
/// `print(1::number)`, which have to parse back.
#[test]
fn any_simple_expression_takes_a_type_cast() {
    let allocator = Allocator::new();
    for source in [
        "local x = 1 :: number",
        "local x = \"s\" :: string",
        "local x = {} :: Foo",
        "local x = true :: boolean",
        "print(1 :: number)",
        "local x = f() :: T",
    ] {
        let result = parse_str(&allocator, source);
        assert_no_errors(&result);
    }
}

#[test]
fn type_cast_binds_tighter_than_binary_op() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "local x = a + b :: number");
    assert_no_errors(&result);
    let operands = run_operands(local_value(&result));
    assert!(
        matches!(operands[1], Expression::TypeCast(_)),
        "cast must bind to the right operand: a + (b :: number)"
    );
}

#[test]
fn a_second_cast_needs_parentheses() {
    let allocator = Allocator::new();
    // Grammar: asexp ::= simpleexp ['::' Type], one cast per simpleexp.
    // Luau rejects a second `::`; chaining needs parens.
    let bare = parse_str(&allocator, "local x = v :: A :: B");
    assert!(!bare.errors.is_empty(), "chained casts must error");
    assert_no_errors(&parse_str(&allocator, "local x = (v :: A) :: B"));
}

#[test]
fn nested_interpolated_string_stmt_is_an_error() {
    let allocator = Allocator::new();
    // A bare string isn't a statement, so this has to report errors, and the
    // nesting mustn't make the parser panic.
    let source = "`outer{`inner{x}`}`";
    let result = parse_str(&allocator, source);
    assert!(
        !result.errors.is_empty(),
        "an interpolated string isn't a statement; expected parse errors"
    );
}

#[test]
fn ampersand_pipe_and_tilde_are_no_expression_operators() {
    let allocator = Allocator::new();
    // `&` and `|` are type operators, and `~` only starts a `~=`.
    for source in
        ["local a = 1 & 2", "local a = 1 | 2", "return x & y", "return x | y", "return ~x"]
    {
        let result = parse_str(&allocator, source);
        assert!(!result.errors.is_empty(), "must reject {source}");
    }
    let result = parse_str(
        &allocator,
        "\
type U = number | string
type I = A & B",
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}

#[test]
fn empty_interpolation_rejected() {
    let allocator = Allocator::new();
    for (source, hole) in [
        ("local s = `a{}b`", "{}"),
        ("local s = `{x}a{}b`", "{}"),
        ("local s = `{}a{x}b`", "{}"),
        ("local s = `{x}a{ --[[c]] }b`", "{ --[[c]] }"),
    ] {
        let result = parse_str(&allocator, source);
        let [error] = result.errors.as_slice() else {
            panic!("{source}: {:?}", result.errors);
        };
        assert_eq!(
            (&*error.message, error.span.unwrap().text(source.as_bytes())),
            ("malformed interpolated string: expected expression inside `{}`", hole.as_bytes()),
            "{source}"
        );
    }
    // Plain backtick strings still parse.
    assert_no_errors(&parse_str(&allocator, "return `plain`"));
    assert_no_errors(&parse_str(&allocator, "return ``"));
    assert_no_errors(&parse_str(&allocator, "return `a{1}b`"));
    let unterminated = parse_str(&allocator, "local s = `plain");
    assert_eq!(
        unterminated.errors.iter().map(|e| &*e.message).collect::<Vec<_>>(),
        ["unterminated interpolated string"]
    );
}

#[test]
fn an_unterminated_interpolated_string_is_one_error() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "\
local s = `{x}a
local after = 1",
    );
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.message, "unterminated interpolated string");
    let [Statement::LocalAssignment(_), Statement::LocalAssignment(after)] = &*result.chunk.stmts
    else {
        panic!("{:?}", result.chunk.stmts);
    };
    assert_eq!(after.names.first().map(|name| name.name.text.as_str()), Some("after"));
}

#[test]
fn integer_literals_are_integer_nodes_within_64_bits() {
    let allocator = Allocator::new();
    let result = parse_str(
        &allocator,
        "return 1i, 1_000_000i, 1i_, 0xffffffffffffffffi, \
         0b1000000000000000000000000000000000000000000000000000000000000000i",
    );
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.stmts else {
        panic!("expected return");
    };
    assert!(return_stmt.exprs.iter().all(|expr| matches!(expr, Expression::Integer(_))));

    for source in [
        "return 9223372036854775808i",
        "return 0x10000000000000000i",
        "return 123ii",
        "return 123.0i",
        "return 0b250i",
    ] {
        assert!(!parse_str(&allocator, source).errors.is_empty(), "{source} must be rejected");
    }

    // The suffix binds to the literal, not the unary minus, so the
    // decimal boundary value overflows even when negated; i64::MIN is
    // only spellable as a hex bit pattern.
    assert_no_errors(&parse_str(&allocator, "return -123i"));
    assert_no_errors(&parse_str(&allocator, "return 0x8000000000000000i"));
    assert!(
        !parse_str(&allocator, "return -9223372036854775808i").errors.is_empty(),
        "decimal i64::MIN spelling must overflow"
    );
}

#[test]
fn explicit_type_instantiation_is_a_suffix() {
    let allocator = Allocator::new();
    let result = parse_str(&allocator, "return module.f<<number, string>>(value)");
    assert_no_errors(&result);
    let [Statement::Return(return_stmt)] = &*result.chunk.stmts else {
        panic!("expected return");
    };
    let call = chain(return_stmt.exprs.first().expect("a returned value"));
    assert_eq!(chain_shape(call), "module.f<<>>()");
    let Some(Suffix::TypeInstantiation { type_args, .. }) = call.suffixes.get(1) else {
        panic!("expected explicit type instantiation");
    };
    assert_eq!(type_args.args.len(), 2);

    for source in [
        "return f<<number>>",
        "return t.f<<number>>()",
        "return t:f<<number>>()",
        "return t[\"f\"]<<number>>()",
    ] {
        assert_no_errors(&parse_str(&allocator, source));
    }

    let method = parse_str(&allocator, "return t:f<<number, string>>()");
    let [Statement::Return(method_return)] = &*method.chunk.stmts else {
        panic!("expected return");
    };
    let method_chain = chain(method_return.exprs.first().expect("a returned value"));
    let Suffix::MethodCall(method_call) = method_chain.suffixes.last() else {
        panic!("expected method call");
    };
    assert_eq!(method_call.type_args.as_ref().expect("method type arguments").args.len(), 2);
}

#[test]
fn a_call_whose_parenthesis_starts_a_line_is_ambiguous() {
    let allocator = Allocator::new();
    assert_has_errors(&parse(
        &allocator,
        b"\
a = b
(f)(g)",
    ));
    // Same line: fine.
    assert_no_errors(&parse(&allocator, b"a = b(f)(g)"));
    // Non-paren call args never trigger the check.
    assert_no_errors(&parse(
        &allocator,
        b"\
a = b
{ x = 1 }",
    ));
}

#[test]
fn ambiguous_newline_check_covers_method_calls() {
    let allocator = Allocator::new();
    // `parseFunctionArgs` applies the line check to `a:b(...)` too.
    let result = parse(
        &allocator,
        b"\
a:b
(c)",
    );
    assert!(
        result.errors.iter().any(|e| e.message.contains("ambiguous")),
        "must reject a method call whose `(` starts a new line: {:?}",
        result.errors
    );
}

#[test]
fn a_lone_carriage_return_does_not_make_a_call_ambiguous() {
    let allocator = Allocator::new();
    // A CR is whitespace to Luau's lexer, which breaks lines at LF.
    let result = parse(&allocator, b"f\r(x)");
    assert!(result.errors.is_empty(), "luau accepts this program: {:?}", result.errors);
}
