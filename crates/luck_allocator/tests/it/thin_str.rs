use std::{mem, ptr};

use luck_allocator::{Allocator, ThinStr};
use oxc_data_structures::types::implements;

#[test]
fn holds_a_str_in_three_words_of_four_bytes() {
    assert_eq!(mem::size_of::<ThinStr<'_>>(), 12);
    assert_eq!(mem::align_of::<ThinStr<'_>>(), 4);
    assert_eq!(mem::size_of::<Option<ThinStr<'_>>>(), 12);
}

#[test]
fn gives_back_the_str_it_was_made_from() {
    let allocator = Allocator::new();
    let text = allocator.alloc_str("name");
    let thin = ThinStr::new(text);
    assert!(ptr::eq(thin.as_str(), text));
    assert_eq!(thin.len(), 4);
    assert_eq!(ThinStr::new("").as_str(), "");
}

#[test]
fn compares_and_prints_as_its_str() {
    let thin = ThinStr::from("name");
    assert_eq!(thin, ThinStr::new(&String::from("name")));
    assert_eq!(thin, "name");
    assert_ne!(thin, "other");
    assert_eq!(format!("{thin} {thin:?}"), "name \"name\"");
}

#[test]
fn is_send_and_sync() {
    assert!(implements!(ThinStr<'static>: Send));
    assert!(implements!(ThinStr<'static>: Sync));
}
