//! Inlining a local read once: its value, when it's the same wherever it
//! runs, moves to the read, which the printer parenthesizes as it needs.
//!
//! The walk takes the value as it leaves the `local`, puts it in place of
//! the read when it reaches the read, and settles the `local` as it leaves
//! the block that declares it: the name goes once the value has moved or
//! the read has gone with code a rewrite dropped, and the value goes back
//! where the read can't take it.

use std::mem;

use luck_allocator::List;
use luck_ast::{
    Expression, LocalAssignment, MAX_NESTING_DEPTH, Name, Statement, VisitMut as _,
    expr_nesting_depth,
};
use luck_semantic::{ReferenceId, SymbolId};
use luck_token::MAX_ACTIVE_LOCALS;
use luck_value::IsLiteralValue as _;

use super::remove_dead_code::is_nil_initialized;
use super::simplify_parens::Place;
use super::{Compressor, remove_stmts};
use crate::binding::has_fixed_binding;
use crate::expr::{MAX_ADDED_DEPTH, parenthesize};

/// A symbol's place in the compressor's held values while it holds none.
pub(super) const NOT_HELD: u32 = u32::MAX;

/// An inlined local's initializer, from the `local` the walk took it from
/// until the end of the block that declares it.
pub(super) struct Held<'ast> {
    symbol: SymbolId,
    read: ReferenceId,
    value: Expression<'ast>,
    /// The inlined locals it took in, its own included. Each is still in
    /// scope where its local is declared, so a chain past
    /// [`MAX_ACTIVE_LOCALS`] of them is longer than one function can hold,
    /// and it stops there.
    locals: usize,
    /// How deep the value nests.
    depth: usize,
    state: HeldState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeldState {
    /// Waits for its read, which a rewrite may drop before the walk reaches
    /// it.
    Waiting,
    Landed,
    /// The read sits where the value can't go: past the nesting cap, or
    /// where a name stands outside an expression, as in `function t.f()`.
    Refused,
}

impl<'ast> Compressor<'_, 'ast> {
    /// Walks a `local`, drops a `nil` initializer, and takes the value of
    /// each name read once whose value can move to the read, in a statement
    /// that gives each name its own value.
    pub(super) fn visit_local(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::LocalAssignment(local) = stmt else {
            return true;
        };
        // An initializer can hold `local`s of its own, so the list is a
        // stack that every `local` leaves as it found it.
        let landed_start = self.landed_counts.len();
        for attributed in &mut local.names {
            self.visit_attributed_name(attributed);
        }
        let (name_count, value_count) = (local.names.len(), local.exprs.len());
        for (index, expr) in local.exprs.iter_mut().enumerate() {
            let outer_landed_locals = mem::replace(&mut self.landed_locals, 0);
            let place = self.list_item(index, value_count, name_count > value_count);
            self.visit_expr_in(expr, place);
            let landed_locals = mem::replace(&mut self.landed_locals, outer_landed_locals);
            self.landed_counts.push(landed_locals);
        }
        if self.config.remove_dead_code && is_nil_initialized(local) {
            local.exprs = List::new();
            self.has_changed = true;
        }
        if self.config.inline_locals
            && !has_fixed_binding(local, self.version)
            && local.names.len() == local.exprs.len()
        {
            let LocalAssignment { names, exprs, .. } = &mut **local;
            for (index, (attributed, value)) in names.iter().zip(exprs).enumerate() {
                let landed_locals = self.landed_counts[landed_start + index];
                if let Some(symbol) = self.scoping.symbol_of(&attributed.name) {
                    self.hold(symbol, value, landed_locals);
                }
            }
        }
        self.landed_counts.truncate(landed_start);
        true
    }

    /// The one live reference to `symbol`, when it's a read.
    fn single_read(&self, symbol: SymbolId) -> Option<ReferenceId> {
        let mut read = None;
        for &id in self.scoping.resolved_references(symbol) {
            if self.dropped.contains(id.index()) {
                continue;
            }
            if self.scoping.reference(id).kind.is_write() || read.is_some() {
                return None;
            }
            read = Some(id);
        }
        read
    }

    /// Takes `value`, the initializer of the local `symbol`, to wait for the
    /// local's one read, if it has exactly one and no write, and the value
    /// can move there, having taken in the values of `landed_locals` locals.
    fn hold(&mut self, symbol: SymbolId, value: &mut Expression<'ast>, landed_locals: usize) {
        if landed_locals >= MAX_ACTIVE_LOCALS || !value.is_literal_value(&self.names()) {
            return;
        }
        let Some(read) = self.single_read(symbol) else {
            return;
        };
        let depth = expr_nesting_depth(value);
        let index = u32::try_from(self.held.len()).unwrap_or(NOT_HELD);
        if depth > MAX_ADDED_DEPTH || index == NOT_HELD {
            return;
        }
        self.held_index[symbol.index()] = index;
        let locals = landed_locals + 1;
        let state = HeldState::Waiting;
        self.held.push(Held { symbol, read, value: value.take(), locals, depth, state });
        self.waiting += 1;
    }

    /// The place among the held values, from `start` on, of the local `name`
    /// declares, if it holds one.
    fn held_of(&self, name: &Name<'_>, start: usize) -> Option<usize> {
        let symbol = self.scoping.symbol_of(name)?;
        let held = self.held_index[symbol.index()] as usize;
        (start..self.held.len()).contains(&held).then_some(held)
    }

    /// The waiting value of the local `name` reads, if there is one.
    fn waiting_value(&mut self, name: &Name<'_>) -> Option<&mut Held<'ast>> {
        let symbol = self.resolved(name)?;
        let index = self.held_index[symbol.index()] as usize;
        self.held.get_mut(index).filter(|held| held.state == HeldState::Waiting)
    }

    /// The value of the local `name` reads, to put in its place, if it
    /// waits for this read and fits where it lands. A Luau integer it is
    /// lands in parentheses under `-`.
    pub(super) fn take_held(&mut self, name: &Name<'_>) -> Option<Expression<'ast>> {
        let depth = self.depth;
        let held = self.waiting_value(name)?;
        let fits = depth + 1 + held.depth <= MAX_NESTING_DEPTH;
        held.state = if fits { HeldState::Landed } else { HeldState::Refused };
        let (read, locals) = (held.read, held.locals);
        let value = fits.then(|| held.value.take());
        self.waiting -= 1;
        if value.is_some() {
            self.mark_dropped(read);
            self.landed_locals += locals;
            self.has_changed = true;
        }
        value
    }

    /// Keeps the value of the local `name` reads where it is.
    pub(super) fn refuse_held(&mut self, name: &Name<'_>) {
        if let Some(held) = self.waiting_value(name) {
            held.state = HeldState::Refused;
            self.waiting -= 1;
        }
    }

    /// Settles each value taken from a `local` of `stmts`, from `start` of
    /// the held values on: the name of one that landed, or whose read went,
    /// goes, and one the read refused goes back.
    pub(super) fn settle_held(&mut self, stmts: &mut List<'ast, Statement<'ast>>, start: usize) {
        if self.held.len() <= start {
            return;
        }
        let mut removed = mem::take(&mut self.removed_stmts);
        removed.clear();
        for (stmt_index, stmt) in stmts.iter_mut().enumerate() {
            let Statement::LocalAssignment(local) = stmt else {
                continue;
            };
            // Only a `local` that gives each name its own value holds any.
            if local.names.iter().all(|attributed| self.held_of(&attributed.name, start).is_none())
            {
                continue;
            }
            let mut kept = 0;
            for index in 0..local.names.len() {
                let place = self.held_of(&local.names[index].name, start);
                let state = place.map(|at| self.held[at].state);
                if matches!(state, Some(HeldState::Landed | HeldState::Waiting)) {
                    self.drop_annotation(&local.names[index]);
                    self.has_changed = true;
                    continue;
                }
                if let Some(refused) = place {
                    local.exprs[index] = self.held[refused].value.take();
                }
                local.names.swap(kept, index);
                local.exprs.swap(kept, index);
                kept += 1;
            }
            if kept == 0 {
                removed.push(stmt_index);
            }
            local.names.truncate(kept);
            local.exprs.truncate(kept);
        }
        remove_stmts(stmts, &removed);
        self.removed_stmts = removed;
        for held in self.held.drain(start..) {
            self.held_index[held.symbol.index()] = NOT_HELD;
            // A value whose read went with dropped code never lands.
            if held.state == HeldState::Waiting {
                debug_assert!(self.dropped.contains(held.read.index()), "the walk missed a read");
                self.waiting -= 1;
            }
        }
    }

    /// Walks an operand at `place`, where a value that lands takes no
    /// parentheses but the ones that keep its meaning.
    pub(super) fn visit_operand(&mut self, operand: &mut Expression<'ast>, place: Place) {
        if let Expression::Name(name) = operand
            && self.waiting > 0
            && let Some(value) = self.take_held(name)
        {
            self.has_landed_run |= matches!(value, Expression::BinaryOp(_));
            // Luau: `-1i` is a negative integer literal, while `-x` negates
            // the integer `x` holds, which raises.
            *operand = if place == Place::Negated && matches!(value, Expression::Integer(_)) {
                parenthesize(self.allocator, value)
            } else {
                value
            };
            return;
        }
        self.visit_expr_in(operand, place);
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_ast::{Statement, expr_nesting_depth};
    use luck_codegen::LiteralSpelling;
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, assert_outputs, compressed, minify_with, walk_once};
    use crate::transform_config::TransformConfig;

    const INLINING: TransformConfig = TransformConfig { inline_locals: true, ..NO_PASSES };

    fn inline(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &INLINING)
    }

    fn inline_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &INLINING)
    }

    #[test]
    fn inlines_single_use_literals() {
        assert_outputs(
            inline,
            &[
                ("local x = 42 return x", "return 42"),
                ("local x = 42 local y = x + 1 return y", "return 42+1"),
                (
                    "local function foo() local x = 42 return x end return foo",
                    "local function foo()return 42 end return foo",
                ),
                ("if true then local x = 42 print(x) end", "if true then print(42)end"),
                ("local x = 1 print(x)", "print(1)"),
                ("local x = 1 local function f() return x end", "local function f()return 1 end"),
                ("do local x = 1 do print(x) end end", "do do print(1)end end"),
                ("repeat local x = 1 until x == y", "repeat until 1==y"),
                ("repeat local x = 1 repeat until x until z", "repeat repeat until 1 until z"),
                ("local x, y = 1, 2 print(y, x)", "print(2,1)"),
                ("local a, b, c = 1, f(), 3 print(c, b)", "local a,b=1,f()print(3,b)"),
            ],
        );
    }

    #[test]
    fn keeps_what_inlining_would_change() {
        for source in [
            // More than one read.
            "local x=42 print(x)return x",
            // A call, a read, a table, a closure, and `...` each carry an
            // effect or an identity.
            "local x=foo()return x",
            "local x=y return x",
            "local t={1,2,3}return t",
            "local f=function()return 1 end return f",
            "local function f(...)local x=...return x end",
            // A read outside the declaration's scope is a global's.
            "print(x)local x=1",
            "do local x=1 end print(x)",
            // A write would land on a global.
            "local x=1 x=2",
            "local x=\"s\"function x.f()end print(x)",
            // A function statement reads the name it stores into, which can't
            // be a literal.
            "local x=\"s\"function x.f()end",
            // The error must still fire before the print.
            "local x=1+nil print(\"before\")return x",
        ] {
            assert_eq!(inline(source), source);
        }
        assert_eq!(inline_luau("local x=1 x+=2"), "local x=1 x+=2");
        assert_eq!(inline_luau("local x=foo::Bar return x"), "local x=foo::Bar return x");
    }

    #[test]
    fn keeps_a_name_another_binding_wears() {
        assert_eq!(
            inline("local self = 5 function M:get() return self end"),
            "local self=5 function M:get()return self end"
        );
        assert_eq!(
            minify_with(
                "local arg = 5 local function f(...) return arg end return f",
                LuaVersion::Lua51,
                &INLINING
            ),
            "local arg=5 local function f(...)return arg end return f"
        );
        for source in [
            "local args=1 local function f(...args)return args end",
            "local x=1 global x print(x)",
            "local x=1 global function g()local x=2 return x end return g",
        ] {
            let result = minify_with(source, LuaVersion::Lua55, &INLINING);
            assert!(!result.contains("return 1"), "{source}: {result}");
        }
    }

    #[test]
    fn inlines_each_of_two_locals_that_share_a_name() {
        assert_eq!(
            walk_once("local x = 1 print(x) do local x = 2 print(x) end", INLINING),
            "print(1)do print(2)end"
        );
    }

    #[test]
    fn inlines_a_local_whose_other_reads_a_rewrite_drops() {
        let both = TransformConfig { remove_dead_code: true, fold_constants: true, ..INLINING };
        for (source, expected) in [
            ("local x = 1 print(x) if false then print(x) end", "print(1)"),
            ("local x = 1 if false then print(x) end print(x)", "print(1)"),
            ("local x = 1 print(x) print(false and x)", "print(1)print(false)"),
            ("local x = 1 print(x) local function f() return x end", "print(1)"),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Lua54, &both), expected, "{source}");
        }
    }

    #[test]
    fn inlines_a_local_whose_value_takes_a_late_inlined_one() {
        let both = TransformConfig { remove_dead_code: true, ..INLINING };
        assert_eq!(
            minify_with(
                "local a = 6 local b = a if false then print(a) end print(b)",
                LuaVersion::Lua54,
                &both
            ),
            "print(6)"
        );
    }

    #[test]
    fn drops_the_annotation_of_a_name_it_inlines() {
        let both = TransformConfig { remove_dead_code: true, ..INLINING };
        for (source, config) in [
            ("local a = 5 local b: typeof(a) = 1 print(a, b)", INLINING),
            ("local a = 5 local b: typeof(a) = 1 if false then print(b) end print(a, b)", both),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Luau, &config), "print(5,1)", "{source}");
        }
    }

    #[test]
    fn parenthesizes_a_value_that_would_regroup_where_it_lands() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 / 0 return 3 % s", "return 3%(1/0)"),
                ("local s = 1 + 2 return s * 2", "return(1+2)*2"),
                ("local s = 1 + 2 return 2 * s", "return 2*(1+2)"),
                ("local s = 1 - 2 return 5 - s", "return 5-(1-2)"),
                ("local s = 2 ^ 3 return s ^ 2", "return(2^3)^2"),
                ("local s = 1 .. 2 return s .. 3", "return(1 ..2)..3"),
                ("local s = 1 + 2 return 2 ^ s", "return 2^(1+2)"),
                ("local s = 1 or 2 return s and 3", "return(1 or 2)and 3"),
                ("local s = -2 return s ^ y", "return(-2)^y"),
                ("local s = not nil return s ^ y", "return(not nil)^y"),
                ("local s = 1 + 2 return -s", "return-(1+2)"),
                ("local s = 1 == 2 return not s", "return not(1==2)"),
                ("local s = \"a\" .. \"b\" return #s", "return#(\"a\"..\"b\")"),
                ("local s = 1 + 2 return -s ^ 2", "return-(1+2)^2"),
                ("local s = -1 return -s ^ 2", "return-(-1)^2"),
                ("local s = 1 + 2 return (s * 3)", "return(1+2)*3"),
                ("local s = 1 + 2 print(s * 3, 4)", "print((1+2)*3,4)"),
                ("local a = 1 + 2 local b = a * 3 return b ^ 2", "return((1+2)*3)^2"),
                ("local x = 37.5 return x[\"k\"]", "return(37.5)[\"k\"]"),
                ("local x = \"s\" return x.len", "return(\"s\").len"),
                ("local x = false x:resize(9)", "(false):resize(9)"),
            ],
        );
    }

    #[test]
    fn leaves_a_value_bare_where_its_grouping_holds() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 + 2 return s", "return 1+2"),
                ("local s = 1 + 2 print(s)", "print(1+2)"),
                ("local s = 1 + 2 return {s}", "return{1+2}"),
                ("local s = 1 + 2 return t[s]", "return t[1+2]"),
                ("local s = 1 * 2 return s + 3", "return 1*2+3"),
                ("local s = 1 * 2 return 3 + s", "return 3+1*2"),
                ("local s = 1 - 2 return s - 5", "return 1-2-5"),
                ("local s = 2 ^ 3 return 2 ^ s", "return 2^2^3"),
                ("local s = 1 .. 2 return 3 .. s", "return 3 ..1 ..2"),
                ("local s = 1 + 2 return s < 4", "return 1+2<4"),
                ("local s = 1 < 2 return s == true", "return 1<2==true"),
                ("local s = 2 ^ 3 return -s", "return-2^3"),
                ("local s = -2 return 3 ^ s", "return 3^-2"),
                ("local s = -2 return s * 3", "return-2*3"),
                ("local s = -1 return -s", "return- -1"),
                ("local s = (1 + 2) return s * 2", "return(1+2)*2"),
            ],
        );
    }

    #[test]
    fn inlines_into_luau_casts_and_instantiations() {
        assert_outputs(
            inline_luau,
            &[
                ("local s = 1 + 2 return s :: any", "return(1+2)::any"),
                ("local s = -1 return s :: any", "return(-1)::any"),
                ("local s = 1 :: number return s :: any", "return(1::number)::any"),
                ("local s = 1 return s :: any", "return 1::any"),
                ("local s = 1 :: number return s < 2", "return(1::number)<2"),
                ("local s = 1 :: number return 2 + s < 3", "return 2+(1::number)<3"),
                ("local s = 1 :: number return -s < 2", "return-(1::number)<2"),
                ("local s = 1 :: number return 1 < s < 3", "return 1<(1::number)<3"),
                ("local s = 2 + 1 :: number return s < 3", "return 2+(1::number)<3"),
                ("local s = -1 :: number return s < 3", "return-(1::number)<3"),
                // The cast ends inside the sum's parentheses, before `)`.
                ("local s = 2 + 1 :: number return 2 * s < 3", "return 2*(2+1::number)<3"),
                ("local s = 1 :: number return s", "return 1::number"),
                ("local s = 1 :: number return s + 1", "return 1::number+1"),
                ("local s = 1 :: number return 1 + s", "return 1+1::number"),
                ("local s = 1 :: number return s > 2", "return 1::number>2"),
                ("local s = 1 return x :: Foo<T> < s", "return x::Foo<T><1"),
                ("local s = \"x\" return s<<T>>", "return(\"x\")<<T>>"),
                ("local s = -1 return s<<T>>", "return(-1)<<T>>"),
                ("local s = \"x\" :: T return s<<U>>", "return(\"x\"::T)<<U>>"),
                ("local s = \"x\" return s<<T>><<U>>", "return(\"x\")<<T>><<U>>"),
                // `-1i` is a literal, while negating an integer raises.
                ("local s = 1i return -s", "return-(1i)"),
                ("local s = 1i return not s", "return not 1i"),
                ("local s = (\"x\")<<T>> return s .. \"y\"", "return(\"x\")<<T>>..\"y\""),
            ],
        );
    }

    /// `local x0 = 1`, then `local xi = x(i-1) + 1` up to `x(links)`, returned.
    fn sum_chain(links: usize) -> String {
        let mut source = String::from("local x0 = 1\n");
        for i in 1..=links {
            writeln!(source, "local x{i} = x{} + 1", i - 1).unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        source
    }

    #[test]
    fn inlines_a_chain_of_as_many_locals_as_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS - 1;
        let allocator = Allocator::new();
        let source = sum_chain(links);
        let chunk = compressed(&allocator, &source, INLINING);
        let [Statement::Return(return_stmt)] = &*chunk.block.stmts else {
            panic!("expected a return");
        };
        let run = return_stmt.exprs.first().expect("the return has its value");
        assert_eq!(expr_nesting_depth(run), 1, "the chain splices into one run");
        assert_eq!(
            luck_codegen::compact(&chunk, LiteralSpelling::Shortest),
            format!("return 1{}", "+1".repeat(links)).into_bytes()
        );

        let mut concat = String::from("local x0 = \"z\"\n");
        for i in 1..=links {
            writeln!(concat, "local x{i} = \"a\" .. x{}", i - 1).unwrap();
        }
        writeln!(concat, "return x{links}").unwrap();
        assert_eq!(inline(&concat), format!("return{}\"z\"", "\"a\"..".repeat(links)));
    }

    #[test]
    fn stops_a_chain_past_the_locals_one_function_holds_in_one_pass() {
        let links = luck_token::MAX_ACTIVE_LOCALS;
        let expected = format!("local x{links}=1{} return x{links}", "+1".repeat(links));
        assert_eq!(walk_once(&sum_chain(links), INLINING), expected);
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let mut source = String::from("local x0 = 1\n");
        for i in 1..luck_token::MAX_ACTIVE_LOCALS {
            writeln!(source, "local x{i} = x{} * 2 + 1", i - 1).unwrap();
        }
        writeln!(source, "return x{}", luck_token::MAX_ACTIVE_LOCALS - 1).unwrap();
        assert!(inline(&source).contains("local"), "the run must stop inlining partway");
    }
}
