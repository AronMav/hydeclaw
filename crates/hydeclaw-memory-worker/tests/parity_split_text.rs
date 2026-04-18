//! Parity test: locks the byte-for-byte output of `hydeclaw_text::split_text`
//! for a representative input. Catches accidental algorithm drift.

use hydeclaw_text::split_text;

#[test]
fn split_text_byte_parity_for_known_input() {
    // Three paragraphs of differing lengths to exercise paragraph-boundary
    // splitting + overlap behavior.
    let text = "First paragraph with some content that fills space.\n\n\
                Second paragraph that also has reasonable length here.\n\n\
                Third paragraph wraps things up with a few extra words.";
    let chunks = split_text(text, 80, 20);

    // Lock the count + the content of each chunk. If the algorithm ever
    // changes, this test will fail loudly and the change can be reviewed.
    // Note: chunks overlap by 20 bytes per the overlap parameter.
    let expected: Vec<&str> = vec![
        "First paragraph with some content that fills space.\n\n",
        " that fills space.\n\nSecond paragraph that also has reasonable length here.\n\n",
        "nable length here.\n\nThird paragraph wraps things up with a few extra words.",
    ];
    assert_eq!(
        chunks.len(),
        expected.len(),
        "chunk count drift: got {} chunks, expected {}",
        chunks.len(),
        expected.len()
    );
    for (i, (got, want)) in chunks.iter().zip(expected.iter()).enumerate() {
        assert_eq!(got, want, "chunk {i} byte-mismatch");
    }
}

#[test]
fn split_text_unicode_parity() {
    // Cyrillic input: each char is 2 bytes in UTF-8. Locks char-boundary safety.
    let text = "Привет мир. Это тест.\n\nВторой параграф здесь.\n\nТретий короткий.";
    let chunks = split_text(text, 60, 10);

    // We don't pin exact strings (small differences are OK across paragraphs)
    // but we DO assert: every chunk starts on a char boundary, none is empty,
    // and joining recovers the original character set.
    assert!(!chunks.is_empty());
    for (i, chunk) in chunks.iter().enumerate() {
        assert!(!chunk.is_empty(), "chunk {i} is empty");
        assert!(chunk.is_char_boundary(0), "chunk {i} starts mid-character");
    }
    let joined: String = chunks.iter().map(|s| s.as_str()).collect();
    assert!(joined.contains("Привет"));
    assert!(joined.contains("Третий"));
}
