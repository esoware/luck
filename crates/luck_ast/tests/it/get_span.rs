use luck_allocator::{Allocator, Box};
use luck_ast::{Expression, Field, GetSpan as _, Name, Parameter, Type};
use luck_token::Span;

fn name(text: &'static str, start: u32) -> Name<'static> {
    let end = start.saturating_add(1);
    Name::new(text.into(), None, Span::new(start, end))
}

#[test]
fn a_node_without_a_span_spans_its_fields() {
    let allocator = Allocator::new();
    // x: T, then x alone
    let annotation = Type::new_name("T".into(), None, Span::new(3, 4), &allocator);
    let typed = Parameter::new(name("x", 0), Some(Box::new_in(annotation, &allocator)));
    assert_eq!(typed.span(), Span::new(0, 4));
    assert_eq!(Parameter::new(name("x", 0), None).span(), Span::new(0, 1));
    // k = nil
    let field = Field::Named { name: name("k", 1), value: Expression::Nil(Span::new(5, 8)) };
    assert_eq!(field.span(), Span::new(1, 8));
}

#[test]
fn an_enum_spans_the_node_it_holds() {
    let allocator = Allocator::new();
    let nil = Span::new(2, 5);
    assert_eq!(Expression::Nil(nil).span(), nil);
    let named = Expression::new_name("a".into(), None, Span::new(7, 8), &allocator);
    assert_eq!(named.span(), Span::new(7, 8));
}
