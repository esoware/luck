use std::ptr;

use luck_allocator::{Allocator, TextBytes, ThinStr};

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
fn ascii_text_bytes_give_the_text_of_any_range_in_them() {
    let bytes = TextBytes::new(b"local name");
    assert_eq!(bytes.text(6..10).map(ThinStr::as_str), Some("name"));
    assert_eq!(bytes.text(0..0).map(ThinStr::as_str), Some(""));
    assert_eq!(bytes.text(8..20), None);
}

/// A range that starts and ends with an ASCII byte holds whole characters of UTF-8, whatever lies
/// between, and one that doesn't may start or end inside one.
#[test]
fn utf8_text_bytes_give_the_text_of_a_range_with_ascii_ends() {
    let source = "x = \"caf\u{e9}\" .. y";
    let bytes = TextBytes::new(source.as_bytes());
    assert_eq!(bytes.text(0..1).map(ThinStr::as_str), Some("x"));
    assert_eq!(bytes.text(4..11).map(ThinStr::as_str), Some("\"caf\u{e9}\""));
    assert_eq!(bytes.text(15..16).map(ThinStr::as_str), Some("y"));
    assert_eq!(bytes.text(3..3).map(ThinStr::as_str), Some(""));
    for range in [5..9, 8..10, 9..11, 9..10] {
        assert_eq!(bytes.text(range.clone()), None, "{range:?}");
    }
}

/// In bytes that aren't UTF-8, a range is text only when each of its bytes is ASCII.
#[test]
fn other_text_bytes_give_the_text_of_an_ascii_range() {
    let bytes = TextBytes::new(b"local s = 'caf\xE9' .. \xC3\xA9");
    assert_eq!(bytes.text(6..7).map(ThinStr::as_str), Some("s"));
    assert_eq!(bytes.text(11..14).map(ThinStr::as_str), Some("caf"));
    for range in [10..16, 14..15, 20..22, 17..24] {
        assert_eq!(bytes.text(range.clone()), None, "{range:?}");
    }
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
