use core::panic::AssertUnwindSafe;
use std::panic;

use luck_token::CodeBuffer;

#[test]
fn pushes_append_mixed_ascii_and_unicode_in_order() {
    let mut buffer = CodeBuffer::with_capacity(0);
    assert!(buffer.is_empty());
    buffer.print_ascii_byte(b'=');
    buffer.print_str("local x");
    buffer.print_str("\u{00e9}");
    buffer.print_ascii_repeat(b' ', 3);
    assert_eq!(buffer.len(), 13);
    assert_eq!(buffer.into_string(), "=local x\u{00e9}   ");
}

#[test]
fn truncate_drops_trailing_bytes() {
    let mut buffer = CodeBuffer::default();
    buffer.print_str("abc  ");
    assert_eq!(buffer.as_bytes(), b"abc  ");
    buffer.truncate(3);
    assert_eq!(buffer.into_string(), "abc");
}

#[test]
fn truncate_accepts_every_character_boundary() {
    let text = "a\u{e9}\u{6f22}\u{1f600}z";
    for offset in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
        let mut buffer = CodeBuffer::default();
        buffer.print_str(text);
        buffer.truncate(offset);
        assert_eq!(buffer.into_string(), text.split_at(offset).0);
    }
    for offset in [text.len() + 1, usize::MAX] {
        let mut buffer = CodeBuffer::default();
        buffer.print_str(text);
        buffer.truncate(offset);
        assert_eq!(buffer.into_string(), text);
    }
}

#[test]
fn truncate_rejects_splitting_a_character_without_changing_output() {
    let text = "a\u{e9}\u{6f22}\u{1f600}z";
    for offset in (0..text.len()).filter(|offset| !text.is_char_boundary(*offset)) {
        let mut buffer = CodeBuffer::default();
        buffer.print_str(text);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            buffer.truncate(offset);
        }));
        assert!(result.is_err(), "offset {offset}");
        assert_eq!(buffer.into_string(), text);
    }
}

#[test]
fn ascii_pushes_reject_non_ascii_bytes_without_changing_output() {
    let rejects_as_not_ascii = |push: &mut dyn FnMut()| {
        panic::catch_unwind(AssertUnwindSafe(push)).is_err_and(|payload| {
            payload.downcast_ref::<String>().is_some_and(|message| message.contains("not ASCII"))
        })
    };
    for b in [0x80, 0xC3, 0xFF] {
        let mut buffer = CodeBuffer::default();
        buffer.print_str("ok");
        assert!(rejects_as_not_ascii(&mut || buffer.print_ascii_byte(b)), "byte {b}");
        assert!(rejects_as_not_ascii(&mut || buffer.print_ascii_repeat(b, 2)), "byte {b}");
        assert_eq!(buffer.into_string(), "ok");
    }
}
