//! A tree shaped like luck's: a two-word enum whose recursive variants are arena boxes, list fields,
//! and leaves borrowed from the source or copied into the arena.

use std::{mem, thread};

use luck_allocator::{Allocator, Box, List};

use crate::common::{AutoTraits, Fallback as _};

#[derive(Debug, PartialEq, Eq)]
enum Expression<'a> {
    Nil,
    Name(Box<'a, Name<'a>>),
    Call(Box<'a, Call<'a>>),
    Parenthesized(Box<'a, Expression<'a>>),
}

#[derive(Debug, PartialEq, Eq)]
struct Name<'a> {
    text: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
struct Call<'a> {
    callee: Expression<'a>,
    args: List<'a, Expression<'a>>,
}

fn name<'a>(text: &'a str, allocator: &'a Allocator) -> Expression<'a> {
    Expression::Name(Box::new_in(Name { text }, allocator))
}

fn call<'a, const N: usize>(
    callee: &'a str,
    args: [Expression<'a>; N],
    allocator: &'a Allocator,
) -> Expression<'a> {
    let callee = name(callee, allocator);
    Expression::Call(Box::new_in(
        Call { callee, args: List::from_array_in(args, allocator) },
        allocator,
    ))
}

/// `print(x, nil)`, with its names borrowed from `source`.
fn parse<'a>(source: &'a str, allocator: &'a Allocator) -> Expression<'a> {
    let mut scratch = Vec::new();
    scratch.push(name(&source[6..7], allocator));
    scratch.push(Expression::Nil);
    let args = List::split_off_in(&mut scratch, 0, allocator);
    let callee = name(&source[..5], allocator);
    Expression::Call(Box::new_in(Call { callee, args }, allocator))
}

fn args<'tree, 'a>(expr: &'tree Expression<'a>) -> &'tree List<'a, Expression<'a>> {
    let Expression::Call(call) = expr else { panic!("expected a call, found {expr:?}") };
    &call.args
}

const SOURCE: &str = "print(x, nil)";

#[test]
#[cfg(target_pointer_width = "64")]
fn layouts() {
    assert_eq!(size_of::<Expression<'_>>(), 16);
    assert_eq!(size_of::<List<'_, Expression<'_>>>(), 16);
    assert_eq!(size_of::<Option<List<'_, Expression<'_>>>>(), 16);
    assert_eq!(size_of::<Option<Box<'_, Name<'_>>>>(), 8);
}

#[test]
fn trees_compare_by_content() {
    let parsed = Allocator::new();
    let built = Allocator::new();
    let source = String::from(SOURCE);
    let from_source = parse(&source, &parsed);
    let from_code = call("print", [name(built.alloc_str("x"), &built), Expression::Nil], &built);
    assert_eq!(from_source, from_code);
    assert_ne!(from_source, call("print", [Expression::Nil], &built));
}

#[test]
fn mutates_in_place() {
    let allocator = Allocator::new();
    let mut tree = parse(SOURCE, &allocator);
    let Expression::Call(outer) = &mut tree else { unreachable!() };
    let x = mem::replace(&mut outer.args[0], Expression::Nil);
    outer.args[1] = Expression::Parenthesized(Box::new_in(x, &allocator));
    outer.args.swap(0, 1);
    outer.args.truncate(1);
    let expected = call(
        "print",
        [Expression::Parenthesized(Box::new_in(name("x", &allocator), &allocator))],
        &allocator,
    );
    assert_eq!(tree, expected);
}

#[test]
fn trees_from_two_arenas_share_a_list() {
    let first = Allocator::new();
    let x = parse(SOURCE, &first);
    let second = Allocator::new();
    let y = call("f", [], &second);
    let both = List::from_array_in([x, y], &second);
    assert_eq!(args(&both[0]).len(), 2);
    assert!(args(&both[1]).is_empty());
}

#[test]
fn moves_a_tree_to_another_thread() {
    let allocator = Allocator::new();
    let tree = parse(SOURCE, &allocator);
    let tree = thread::scope(|scope| {
        scope
            .spawn(move || {
                assert_eq!(args(&tree).len(), 2);
                tree
            })
            .join()
            .unwrap()
    });
    assert_eq!(tree, parse(SOURCE, &allocator));
}

#[test]
fn reads_a_tree_from_several_threads() {
    let allocator = Allocator::new();
    let tree = parse(SOURCE, &allocator);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert_eq!(args(&tree)[0], name("x", &Allocator::new())));
        }
    });
}

#[test]
fn joins_pieces_built_on_several_threads() {
    let names = ["a", "b", "c"];
    let mut arenas = [Allocator::new(), Allocator::new(), Allocator::new()];
    let pieces: Vec<Expression<'_>> = thread::scope(|scope| {
        let workers: Vec<_> = arenas
            .iter_mut()
            .zip(names)
            .map(|(arena, callee)| {
                scope.spawn(move || {
                    let arena: &Allocator = arena;
                    call(callee, [Expression::Nil], arena)
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });
    let combining = Allocator::new();
    let callee = name("all", &combining);
    let joined = Expression::Call(Box::new_in(
        Call { callee, args: List::from_iter_in(pieces, &combining) },
        &combining,
    ));
    assert_eq!(args(&joined).len(), 3);
    assert_eq!(args(&joined)[2], call("c", [Expression::Nil], &combining));
}

#[test]
fn auto_traits() {
    const {
        assert!(AutoTraits::<Expression<'_>>::IS_SEND);
        assert!(AutoTraits::<Expression<'_>>::IS_SYNC);
        assert!(AutoTraits::<Allocator>::IS_SEND);
        assert!(!AutoTraits::<Allocator>::IS_SYNC);
    }
}
