use std::ptr;

use luck_allocator::{Allocator, AsciiBytes, ThinStr};

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
fn takes_bytes_of_every_short_length_only_when_each_is_ascii() {
    for length in 0..=20 {
        let mut bytes = vec![b'a'; length];
        assert!(ThinStr::from_ascii(&bytes).is_some(), "{length}");
        for i in 0..length {
            bytes[i] = 0x80;
            assert_eq!(ThinStr::from_ascii(&bytes), None, "{length} at {i}");
            bytes[i] = b'a';
        }
    }
}

#[test]
fn ascii_bytes_give_the_text_of_any_range_in_them() {
    assert!(AsciiBytes::new(b"caf\xE9").is_none());
    let bytes = AsciiBytes::new(b"local name").expect("ASCII");
    assert_eq!(bytes.text(6..10), "name");
    assert_eq!(bytes.text(0..0), "");
    assert_eq!(bytes.text(8..20), "");
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
