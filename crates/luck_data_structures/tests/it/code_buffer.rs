use luck_data_structures::CodeBuffer;

#[test]
fn pushes_append_text_and_bytes_that_are_not_utf8_in_order() {
    let mut buffer = CodeBuffer::with_capacity(0);
    assert!(buffer.is_empty());
    buffer.print_byte(b'=');
    buffer.print_bytes(b"local x");
    buffer.print_bytes("\u{00e9}".as_bytes());
    buffer.print_bytes(b"\xE9\xFF");
    buffer.print_byte(0x80);
    buffer.print_repeat(b' ', 3);
    buffer.print_with(|bytes| bytes.extend_from_slice(b"--"));
    assert_eq!(buffer.len(), 18);
    let expected = ["=local x\u{00e9}".as_bytes(), b"\xE9\xFF\x80   --"].concat();
    assert_eq!(buffer.into_bytes(), expected);
}

#[test]
fn pieces_of_every_short_length_append_whole_with_or_without_spare_capacity() {
    let bytes = b"abcdefghijklmnopq\xE9";
    for end in 0..=bytes.len() {
        let piece = bytes.split_at(end).0;
        for capacity in [0, 1, 4, 32] {
            let mut buffer = CodeBuffer::with_capacity(capacity);
            buffer.print_bytes(b"x");
            buffer.print_bytes(piece);
            buffer.print_bytes(b"\xE9");
            let expected = [&b"x"[..], piece, b"\xE9"].concat();
            assert_eq!(buffer.into_bytes(), expected, "capacity {capacity}");
        }
    }
}

#[test]
fn repeats_of_every_count_append_whole_with_or_without_spare_capacity() {
    for count in 0..=40 {
        for capacity in [0, 1, 8, 32, 33, 64] {
            let mut buffer = CodeBuffer::with_capacity(capacity);
            buffer.print_byte(b'x');
            buffer.print_repeat(b'\t', count);
            buffer.print_byte(b'y');
            let expected = [&b"x"[..], &vec![b'\t'; count], b"y"].concat();
            assert_eq!(buffer.into_bytes(), expected, "count {count}, capacity {capacity}");
        }
    }
}

#[test]
fn truncate_drops_trailing_bytes_at_any_offset() {
    let bytes = b"a\xE9\xC3\xA9z";
    for offset in 0..=bytes.len() + 1 {
        let mut buffer = CodeBuffer::default();
        buffer.print_bytes(bytes);
        buffer.truncate(offset);
        assert_eq!(buffer.as_bytes(), bytes.get(..offset).unwrap_or(bytes), "{offset}");
    }
}
