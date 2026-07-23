//! # luck_minifier
//!
//! AST transform pipeline for Lua/Luau minification.
//!
//! Parses source, runs a configurable sequence of transforms, and emits minimal output
//! via [`luck_codegen::compact`]. Each transform is a standalone `fn(Block) -> Block`
//! implementing [`AstTransform`](luck_ast::transform::AstTransform).
//!
//! ## Pipeline Order
//!
//! The authoritative order is the body of [`minify`] - read it there.
//! Structural facts: `fold_constants` runs before AND after
//! `inline_locals` (inlining exposes folds); `remove_dead_code` runs
//! again after the second fold (folds expose dead branches);
//! `rename_locals` is followed by `lift_locals` and a final
//! `merge_locals` that fuses lifted declarations.
//!
//! # Usage
//!
//! ```
//! use luck_core::{LuaTarget, TransformConfig};
//!
//! let output = luck_minifier::minify("return 1 + 2", LuaTarget::Lua54, &TransformConfig::default(), "input.lua").unwrap();
//! assert!(output.len() < "return 1 + 2".len());
//! ```

pub(crate) mod expr;
mod name_gen;
pub(crate) mod tokens;
mod transforms;

use luck_core::TransformConfig;
use luck_core::diagnostics::{Diagnostic, errors};
use luck_core::types::LuaTarget;

/// Convergence cap for the outer pipeline loop and the tail's inner
/// loop. Measured corpora converge in two outer rounds now that the
/// tail fixpoints internally; the cap only bounds pathological
/// oscillation.
const MAX_PIPELINE_ROUNDS: usize = 8;

/// Run the full minification pipeline on Lua source code.
///
/// The transform chain iterates to a FIXPOINT: every pass can expose
/// work for the others (folding opens dead branches, inlining opens
/// folds, lift+merge reshape declarations for the next round). The old
/// hardcoded run-twice schedule demonstrably left bytes on the table -
/// a second `minify` call used to shrink its own output further.
pub fn minify(
    source: &str,
    target: LuaTarget,
    config: &TransformConfig,
    file_path: &str,
) -> Result<String, Vec<Diagnostic>> {
    let version = target.lua_version();

    let result = luck_parser::parse(source, version);
    if !result.errors.is_empty() {
        return Err(result
            .errors
            .iter()
            .map(|e| errors::e008(file_path, e.span.into(), &e.message))
            .collect());
    }
    // Luau hot comments (--!strict, --!native, --!optimize N, ...) set
    // per-file runtime and analysis modes; they only apply before any
    // code, so the leading run must survive minification at the top.
    let hot_comment_prefix = if version.is_luau() {
        let mut prefix = String::new();
        let mut boundary = 0usize;
        for comment in &result.comments {
            let start = comment.span.start as usize;
            let end = comment.span.end as usize;
            if source[boundary..start].chars().any(|c| !c.is_whitespace()) {
                break;
            }
            let text = &source[start..end];
            if let Some(rest) = text.strip_prefix("--")
                && rest.starts_with('!')
            {
                prefix.push_str(text.trim_end());
                prefix.push('\n');
            }
            boundary = end;
        }
        prefix
    } else {
        String::new()
    };
    let mut block = result.block;

    // Outer fixpoint over the full chain. The tail iterates to its own
    // fixpoint inside apply_tail_transforms (rename/lift/merge unblock
    // each other over several steps), so on measured corpora the outer
    // loop converges in two rounds: one that does the work and one that
    // proves it stable. The core passes find nothing after round one -
    // the extra outer rounds only guard cross-stage interactions.
    //
    // Each improving round re-parses its own output before the next one.
    // Transform-built nodes are not always shaped like their parsed
    // equivalents (paren wrappers, span-less tokens), and a pass that
    // matches on the parsed shape can miss work that would be visible
    // after a round trip - the idempotency invariant (minify(minify(x)) ==
    // minify(x)) held only by luck before this. Judging the fixpoint on
    // freshly parsed ASTs makes the emitted text the convergence domain.
    let mut current_source = source.to_string();
    let mut previous_output = luck_codegen::compact(&block, &current_source);
    for _ in 0..MAX_PIPELINE_ROUNDS {
        block = apply_core_transforms(block, config, target);
        block = apply_tail_transforms(block, config, target);
        let output = luck_codegen::compact(&block, &current_source);
        if output == previous_output {
            break;
        }
        previous_output = output;
        current_source.clone_from(&previous_output);
        let reparsed = luck_parser::parse(&current_source, version);
        debug_assert!(
            reparsed.errors.is_empty(),
            "minified round output failed to reparse: {:?}\noutput:\n{current_source}",
            reparsed.errors
        );
        if !reparsed.errors.is_empty() {
            break;
        }
        block = reparsed.block;
    }

    if hot_comment_prefix.is_empty() {
        Ok(previous_output)
    } else {
        Ok(format!("{hot_comment_prefix}{previous_output}"))
    }
}

/// One round of the cheap peephole passes, in dependency order.
fn apply_core_transforms(
    block: luck_ast::shared::Block,
    config: &TransformConfig,
    target: LuaTarget,
) -> luck_ast::shared::Block {
    let version = target.lua_version();

    let block = if config.remove_dead_code {
        transforms::remove_dead_code::remove(block)
    } else {
        block
    };
    let block = if config.simplify_statements {
        transforms::simplify_statements::simplify(block)
    } else {
        block
    };
    let block = if config.fold_constants {
        transforms::fold_constants::fold(block, version)
    } else {
        block
    };
    let block = if config.inline_locals {
        transforms::inline_locals::inline(block)
    } else {
        block
    };
    // Inlining may create new foldable constants...
    let block = if config.fold_constants {
        transforms::fold_constants::fold(block, version)
    } else {
        block
    };
    // ...and folding those (`local DEBUG = false` inlined into
    // `if DEBUG then`) exposes new dead branches within the same round.
    let block = if config.remove_dead_code {
        transforms::remove_dead_code::remove(block)
    } else {
        block
    };
    let block = if config.merge_locals {
        transforms::merge_locals::merge(block)
    } else {
        block
    };
    let block = if config.simplify_indexes {
        transforms::simplify_indexes::simplify(block)
    } else {
        block
    };
    let block = if config.shorten_strings {
        transforms::shorten_strings::shorten(block, version)
    } else {
        block
    };
    let block = if config.shorten_numbers {
        transforms::shorten_numbers::shorten(block, version)
    } else {
        block
    };
    if config.simplify_parens {
        transforms::simplify_parens::simplify(block)
    } else {
        block
    }
}

/// The expensive ordering-sensitive tail: explicit_self feeds the
/// renamer, lift reshapes declarations, and merge fuses what lifting
/// split. Lift decides by name while rename's slot reuse makes distinct
/// bindings share names, so each lift+merge reshape can redistribute
/// names and unblock further lifts; the inner loop runs that cascade to
/// its own fixpoint (judged on emitted text) so the outer loop never
/// pays a core re-run or reparse for tail-only work.
fn apply_tail_transforms(
    block: luck_ast::shared::Block,
    config: &TransformConfig,
    target: LuaTarget,
) -> luck_ast::shared::Block {
    if !config.rename_locals && !config.lift_locals {
        return block;
    }

    // explicit self before rename so the renamer can shorten the parameter
    let mut block = if config.rename_locals {
        transforms::explicit_self::rewrite(block)
    } else {
        block
    };

    // The loop can wander between name/lift configurations instead of
    // improving monotonically; judging by equality alone would then
    // return whatever iteration the cap lands on. Tracking the smallest
    // emit seen makes the result the best configuration visited, not the
    // last - and since the common trajectory ends on its best iteration,
    // the recovery reparse below almost never runs.
    let mut previous_output = luck_codegen::compact(&block, "");
    let mut last_output = String::new();
    let mut best_output = String::new();
    let mut best_len = usize::MAX;
    for _ in 0..MAX_PIPELINE_ROUNDS {
        block = if config.rename_locals {
            transforms::rename_locals::rename(block, target, config.rename_globals)
        } else {
            block
        };

        block = if config.lift_locals {
            transforms::lift_locals::lift(block)
        } else {
            block
        };

        // fuse `local X\nX=Y` back into `local X=Y` after lifting
        block = if config.rename_locals && config.merge_locals {
            transforms::merge_locals::merge(block)
        } else {
            block
        };

        let output = luck_codegen::compact(&block, "");
        if output.len() < best_len {
            best_len = output.len();
            best_output.clone_from(&output);
        }
        if output == previous_output {
            last_output = output;
            break;
        }
        previous_output.clone_from(&output);
        last_output = output;
    }
    if last_output.len() <= best_len {
        return block;
    }
    let reparsed = luck_parser::parse(&best_output, target.lua_version());
    if reparsed.errors.is_empty() {
        reparsed.block
    } else {
        debug_assert!(false, "tail best output failed to reparse: {best_output}");
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_core::transform_config::TransformConfig;

    fn minify_lua54(source: &str) -> String {
        minify(
            source,
            LuaTarget::Lua54,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed")
    }

    fn minify_luau(source: &str) -> String {
        minify(
            source,
            LuaTarget::Luau,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed")
    }

    fn minify_with_config(source: &str, config: &TransformConfig) -> String {
        minify(source, LuaTarget::Lua54, config, "<test>").expect("minify failed")
    }

    fn reparses(source: &str) -> bool {
        let result = luck_parser::parse(source, LuaTarget::Lua54.lua_version());
        result.errors.is_empty()
    }

    #[test]
    fn strips_comments_and_whitespace() {
        let result = minify_lua54("-- comment\nlocal x = 1 -- inline\n--[[ block ]]\nreturn x\n");
        assert!(!result.contains("--"));
        assert!(!result.contains("comment"));
        assert!(!result.contains("block"));
    }

    #[test]
    fn no_double_spaces() {
        let result = minify_lua54("local   x   =   1\nreturn   x\n");
        assert!(!result.contains("  "));
    }

    #[test]
    fn renames_locals() {
        let result = minify_lua54("local longname = 1\nreturn longname\n");
        assert!(!result.contains("longname"));
    }

    #[test]
    fn preserves_globals() {
        let result = minify_lua54("print(42)\n");
        assert!(result.contains("print"));
    }

    #[test]
    fn preserves_method_names() {
        let result = minify_lua54("local t = {}\nt:insert(1)\n");
        assert!(result.contains(":insert"));
    }

    #[test]
    fn preserves_field_names() {
        let result = minify_lua54("local t = { foo = 1 }\nreturn t.foo\n");
        assert!(result.contains("foo"));
    }

    #[test]
    fn shadow_does_not_break_outer_scope() {
        let result = minify_lua54(
            "print(1)\ndo\n  local print = function() end\n  print(2)\nend\nprint(3)\n",
        );
        assert!(
            result.contains("print"),
            "Global print must be preserved, got: {result}"
        );
    }

    #[test]
    fn folds_arithmetic() {
        let result = minify_lua54("local x = 1 + 2\nreturn x\n");
        assert!(result.contains("3"), "Expected folded 3, got: {result}");
        assert!(!result.contains("+"), "Expected no +, got: {result}");
    }

    #[test]
    fn folds_string_concat() {
        let result = minify_lua54("local x = \"hello\" .. \" world\"\nreturn x\n");
        assert!(result.contains("\"hello world\""), "Got: {result}");
    }

    #[test]
    fn folds_not_true() {
        let result = minify_lua54("local x = not true\nreturn x\n");
        assert!(result.contains("false"), "Got: {result}");
    }

    #[test]
    fn hash_string_not_folded() {
        // #"str" must not be folded - escape sequences make raw length unreliable
        let result = minify_lua54("local x = #\"hello\"\nreturn x\n");
        assert!(result.contains("#"), "Got: {result}");
    }

    #[test]
    fn no_fold_division_by_zero() {
        let result = minify_lua54("local x = 1 / 0\nreturn x\n");
        assert!(
            result.contains("/"),
            "Should not fold div by zero: {result}"
        );
    }

    #[test]
    fn shortens_trailing_zero() {
        let result = minify_lua54("local x = 1.0\nreturn x\n");
        assert!(!result.contains("1.0"), "Got: {result}");
    }

    #[test]
    fn shortens_leading_zero() {
        let result = minify_lua54("local x = 0.5\nreturn x\n");
        assert!(result.contains(".5"), "Got: {result}");
    }

    #[test]
    fn shortens_large_round_numbers() {
        // 5.4 has integer/float subtypes: `1e6` is a float, so the
        // integer literal must stay spelled as an integer.
        let result = minify_lua54("local x = 1000000\nreturn x\n");
        assert!(result.contains("1000000"), "Got: {result}");

        // Luau's single number type makes the scientific form safe.
        let result = minify(
            "local x = 1000000\nreturn x\n",
            luck_core::types::LuaTarget::Luau,
            &TransformConfig::default(),
            "test.lua",
        )
        .expect("minify");
        assert!(result.contains("1e6"), "Got: {result}");
    }

    #[test]
    fn removes_parens_around_literal() {
        let result = minify_lua54("local x = (42)\nreturn x\n");
        assert!(!result.contains("(42)"), "Got: {result}");
    }

    #[test]
    fn keeps_parens_around_function_call() {
        // Parens around func call affect multi-return truncation
        let result = minify_lua54("local x = (foo())\nreturn x\n");
        let paren_count = result.matches('(').count();
        assert!(
            paren_count >= 2,
            "Should keep wrapping parens (expected >=2 '(' chars), got {paren_count} in: {result}"
        );
    }

    #[test]
    fn chained_calls_produce_valid_output() {
        let result = minify_lua54("f()\ng()\nh()\n");
        assert!(reparses(&result), "Parse errors: {result}");
        assert!(
            result.contains("f(") && result.contains("g(") && result.contains("h("),
            "All calls must be preserved: {result}"
        );
    }

    #[test]
    fn minus_does_not_become_comment() {
        // `x - -y` must not become `x--y` (that's a comment)
        let result = minify_lua54("local x = 5\nlocal y = 3\nreturn x - -y\n");
        assert!(!result.contains("--"), "Minus became comment: {result}");
    }

    #[test]
    fn and_or_have_spaces() {
        let result = minify_lua54("local x = a and b\nlocal y = a or b\nreturn x, y\n");
        assert!(
            result.contains(" and ") || result.contains(" and\n"),
            "and needs spaces: {result}"
        );
        assert!(
            result.contains(" or ") || result.contains(" or\n"),
            "or needs spaces: {result}"
        );
    }

    #[test]
    fn lua54_const_attribute() {
        let result = minify(
            "local x <const> = 42\nprint(x, x)\n",
            LuaTarget::Lua54,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed");
        assert!(result.contains("<const>"), "Got: {result}");
    }

    #[test]
    fn lua52_goto() {
        let result = minify(
            "goto skip\n::skip::\nlocal x = 1\nprint(x)\n",
            LuaTarget::Lua52,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed");
        assert!(result.contains("goto"), "Got: {result}");
        assert!(result.contains("::"), "Got: {result}");
    }

    #[test]
    fn luau_type_declaration() {
        let result = minify_luau(
            "type Point = { x: number, y: number }\nlocal p: Point = { x = 1, y = 2 }\nreturn p\n",
        );
        assert!(result.contains("type"), "Got: {result}");
    }

    #[test]
    fn luau_if_expression() {
        let result = minify_luau("local x = if true then 1 else 2\nreturn x\n");
        assert!(result.contains("if"), "Got: {result}");
    }

    #[test]
    fn luau_string_interpolation() {
        let result = minify_luau("local name = \"World\"\nlocal g = `Hello, {name}!`\nreturn g\n");
        assert!(result.contains("`"), "Got: {result}");
    }

    #[test]
    fn luau_continue() {
        let result =
            minify_luau("for i = 1, 10 do\n  if i == 5 then continue end\n  print(i)\nend\n");
        assert!(result.contains("continue"), "Got: {result}");
    }

    #[test]
    fn luau_compound_assignment() {
        let result = minify_luau("local x = 0\nx += 1\nreturn x\n");
        assert!(result.contains("+="), "Got: {result}");
    }

    #[test]
    fn luau_merged_rfc_syntax_and_exports_survive_minification() {
        let source = "\
export local public = 129312i
export const mask = 0xffffffffffffffffi
export function apply<T>(value: ~nil)
    return identity<<T>>(value), public, mask
end
";
        let result = minify_luau(source);
        assert!(result.contains("export local public=129312i"), "{result}");
        assert!(
            result.contains("export const mask=0xffffffffffffffffi"),
            "{result}"
        );
        assert!(result.contains("export function apply"), "{result}");
        assert!(result.contains("identity<<T>>"), "{result}");
        assert!(result.contains(":~nil"), "{result}");
        let reparsed = luck_parser::parse(result.clone(), luck_token::LuaVersion::Luau);
        assert!(
            reparsed.errors.is_empty(),
            "minified merged-RFC syntax must reparse: {:?}\n{result}",
            reparsed.errors
        );
    }

    #[test]
    fn disable_rename() {
        let config = TransformConfig {
            rename_locals: false,
            ..Default::default()
        };
        let result = minify_with_config(
            "local longname = 1\nprint(longname)\nreturn longname\n",
            &config,
        );
        assert!(
            result.contains("longname"),
            "Rename should be disabled: {result}"
        );
    }

    #[test]
    fn disable_fold_constants() {
        let config = TransformConfig {
            fold_constants: false,
            ..Default::default()
        };
        let result = minify_with_config("local x = 1 + 2\nreturn x\n", &config);
        assert!(result.contains("+"), "Folding should be disabled: {result}");
    }

    #[test]
    fn function_with_locals_minifies() {
        let src =
            "local function foo(a, b)\n  local c = a + b\n  return c * 2\nend\nreturn foo(1, 2)\n";
        let result = minify_lua54(src);
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(result.len() < src.len(), "Should be shorter: {result}");
        assert!(
            !result.contains("foo"),
            "Local function name 'foo' should be renamed: {result}"
        );
    }

    #[test]
    fn fibonacci_preserves_recursion() {
        let src = "local function fibonacci(n)\n    if n <= 1 then return n end\n    return fibonacci(n - 1) + fibonacci(n - 2)\nend\nlocal results = {}\nfor i = 1, 20 do\n    table.insert(results, fibonacci(i))\nend\nreturn results\n";
        let result = minify_lua54(src);
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(result.contains("table"), "Global 'table' missing: {result}");
        assert!(
            result.contains("insert"),
            "Method 'insert' missing: {result}"
        );
    }

    #[test]
    fn strips_default_for_step() {
        let result = minify_lua54("for i = 1, 10, 1 do print(i) end\n");
        assert!(
            !result.contains(",1 do")
                && !result.contains(",1,")
                && !result.contains(", 1 do")
                && !result.contains(", 1,"),
            "Default step=1 should be removed, got: {result}"
        );
    }

    #[test]
    fn strips_trailing_comma_in_table() {
        let result = minify_lua54("return {1, 2, 3}\n");
        assert!(
            !result.contains(",}") && !result.contains(", }"),
            "Should not have trailing comma, got: {result}"
        );
    }

    #[test]
    fn test_idempotency() {
        let src = "local function add(a, b)\n  return a + b\nend\nlocal x = add(1, 2)\nprint(x)\n";
        let first = minify_lua54(src);
        let second = minify_lua54(&first);
        // Re-minification may rename locals differently (frequency-based),
        // but output length should not grow
        assert!(
            second.len() <= first.len(),
            "Re-minification should not grow output: first={}, second={}",
            first.len(),
            second.len()
        );
        assert!(
            reparses(&second),
            "Re-minified output should re-parse: {second}"
        );
    }

    #[test]
    fn complex_table_code_preserves_semantics() {
        let src = "local function make_table(n)\n    local t = {}\n    for i = 1, n do\n        t[i] = { value = i * 2, label = \"item\" .. tostring(i) }\n    end\n    return t\nend\nlocal items = make_table(10)\nif #items > 5 then\n    print(\"many items\")\nelse\n    print(\"few items\")\nend\nfor _, v in ipairs(items) do\n    if v.value > 10 then\n        print(v.label)\n    end\nend\nreturn items\n";
        let result = minify_lua54(src);
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("tostring"),
            "Global 'tostring' missing: {result}"
        );
        assert!(
            result.contains("ipairs"),
            "Global 'ipairs' missing: {result}"
        );
        assert!(result.contains("print"), "Global 'print' missing: {result}");
        assert!(result.contains("value"), "Field 'value' missing: {result}");
        assert!(result.contains("label"), "Field 'label' missing: {result}");
    }

    #[test]
    fn test_empty_input() {
        let result = minify_lua54("");
        assert!(
            result.is_empty() || result.trim().is_empty(),
            "Empty input should produce empty or near-empty output, got: {result}"
        );
    }

    #[test]
    fn metamethod_safe_dead_code_preserves_variable_ops() {
        // x + 1 is not pure when x could have a side-effectful __add metamethod
        let result = minify_lua54("local unused = x + 1\n");
        assert!(
            result.contains("+"),
            "Should preserve x + 1 (metamethod side effects), got: {result}"
        );
    }

    #[test]
    fn metamethod_safe_dead_code_removes_literal_ops() {
        // 1 + 2 is pure - no metamethods on literal numbers
        let result = minify_lua54("local unused = 1 + 2\n");
        assert!(
            !result.contains("+"),
            "Should remove literal 1 + 2 (no metamethods), got: {result}"
        );
    }

    #[test]
    fn metamethod_safe_truthiness_preserves_variable_ops() {
        // x + 1 is not guaranteed truthy when x could have __add returning nil
        let src = "local x = setmetatable({}, {__add = function() return nil end})\nif true then return x + 1 end\nreturn 0\n";
        let result = minify_lua54(src);
        assert!(
            !result.contains(" and "),
            "Should not simplify to and/or (metamethod may return falsy), got: {result}"
        );
    }

    #[test]
    fn metamethod_safe_truthiness_simplifies_literal_ops() {
        let result = minify_lua54("if true then return 1 + 2 end\nreturn 0\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("3"),
            "1+2 should be folded to 3, got: {result}"
        );
        assert!(
            !result.contains("if"),
            "if true should be eliminated, got: {result}"
        );
    }

    #[test]
    fn upvalue_captured_in_closure_not_broken_by_rename() {
        let src = "local x = 1\nlocal function inner()\n  return x\nend\nreturn inner()\n";
        let result = minify_lua54(src);
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        let inner_result = luck_parser::parse(&result, luck_token::LuaVersion::Lua54);
        assert!(
            inner_result.errors.is_empty(),
            "Minified output must reparse"
        );
    }

    #[test]
    fn concat_right_associativity_preserved() {
        // a .. (b .. c) is right-associative and should NOT become a .. b .. c
        // because Lua concat is right-associative, (a .. b) .. c IS different
        let result = minify_lua54("return (a .. b) .. c\n");
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("("),
            "Parens around left-side concat must be kept (changes associativity): {result}"
        );
    }

    #[test]
    fn dead_local_with_call_extracts_side_effect() {
        let result = minify_lua54("local unused = foo()\n");
        assert!(
            result.contains("foo("),
            "Side-effectful call should be preserved even when result unused: {result}"
        );
        assert!(
            !result.contains("local"),
            "Unused local declaration should be removed: {result}"
        );
    }

    #[test]
    fn vararg_in_last_position_parens_preserved() {
        // (f()) truncates multi-return; parens must be kept
        let result = minify_lua54("return (f())\n");
        assert!(
            result.contains("(f())"),
            "Parens around f() in return must be preserved (multi-return truncation): {result}"
        );
    }

    #[test]
    fn nested_scope_renames_do_not_shadow_upvalues() {
        let src = "local x = 1\ndo\n  local y = x + 1\n  print(y)\nend\nreturn x\n";
        let result = minify_lua54(src);
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("print"),
            "Global 'print' must be preserved: {result}"
        );
    }

    #[test]
    fn fold_exponentiation() {
        let result = minify_lua54("local x = 2 ^ 10\nreturn x\n");
        assert!(
            result.contains("1024"),
            "2^10 should be folded to 1024, got: {result}"
        );
    }

    #[test]
    fn no_fold_overflow_to_inf() {
        let result = minify_lua54("local x = 1e308 * 10\nreturn x\n");
        assert!(result.contains("*"), "Should not fold to inf: {result}");
    }
    #[test]
    fn luau_hot_comments_survive_minification() {
        let output = minify(
            "--!strict
--!native
--!optimize 2
local x: number = 1
return x
",
            LuaTarget::Luau,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed");
        assert!(
            output.starts_with(
                "--!strict
--!native
--!optimize 2
"
            ),
            "hot comments must lead the output: {output}"
        );
        // Idempotent: a second pass keeps exactly one copy.
        let second = minify(
            &output,
            LuaTarget::Luau,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("second minify failed");
        assert_eq!(output, second, "hot comment prefix must be stable");
    }

    #[test]
    fn regular_comments_still_dropped() {
        let output = minify(
            "-- plain comment
return 1
",
            LuaTarget::Luau,
            &TransformConfig::default(),
            "<test>",
        )
        .expect("minify failed");
        assert!(!output.contains("plain comment"), "{output}");
    }
}
