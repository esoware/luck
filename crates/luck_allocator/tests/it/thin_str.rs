use std::ptr;

use luck_allocator::{Allocator, ThinStr};

use crate::common::AutoTraits;

#[test]
fn holds_a_str_in_three_words_of_four_bytes() {
    assert_eq!(size_of::<ThinStr<'_>>(), 12);
    assert_eq!(align_of::<ThinStr<'_>>(), 4);
    assert_eq!(size_of::<Option<ThinStr<'_>>>(), 12);
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
fn takes_bytes_that_are_ascii_and_nothing_else() {
    let bytes = b"name_2";
    let thin = ThinStr::from_ascii(bytes).expect("ASCII is text");
    assert!(ptr::eq(thin.as_bytes(), bytes.as_slice()));
    assert_eq!(ThinStr::from_ascii(b"").map(ThinStr::as_str), Some(""));
    assert_eq!(ThinStr::from_ascii("caf\u{e9}".as_bytes()), None);
    assert_eq!(ThinStr::from_ascii(b"caf\xE9"), None);
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
    const {
        assert!(AutoTraits::<ThinStr<'_>>::IS_SEND);
        assert!(AutoTraits::<ThinStr<'_>>::IS_SYNC);
    }
}
