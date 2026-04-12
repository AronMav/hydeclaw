//! Text chunker for memory indexing.
//! Splits long texts into overlapping chunks by paragraph boundaries.

/// Default max characters per chunk (~1500 bytes ≈ ~400 tokens, safely under 2048 token limit).
pub const DEFAULT_CHUNK_SIZE: usize = 1500;
/// Default overlap in characters between adjacent chunks.
pub const DEFAULT_CHUNK_OVERLAP: usize = 200;

/// Split `text` into chunks of at most `max_chars` with `overlap` chars of overlap.
/// Splits on paragraph boundaries ("\n\n"), falling back to newlines ("\n"),
/// then sentence boundaries (". "), then hard-cut at `max_chars`.
/// Returns original text as single-element vec if it fits in one chunk.
pub fn split_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }
    // Guard: overlap must be less than max_chars to guarantee forward progress
    let overlap = overlap.min(max_chars / 2);

    let separators = ["\n\n", "\n", ". "];
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + max_chars).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }

        if end >= text.len() {
            chunks.push(text[start..].to_string());
            break;
        }

        let slice = &text[start..end];
        let mut split_at = None;
        let mut found_separator = false;

        for sep in &separators {
            if let Some(pos) = slice.rfind(sep) && pos > 0 {
                split_at = Some(start + pos + sep.len());
                found_separator = true;
                break;
            }
        }

        let split_at = split_at.unwrap_or(end);

        chunks.push(text[start..split_at].to_string());

        // Only apply overlap when we found a separator boundary
        if found_separator {
            let new_start = split_at.saturating_sub(overlap);
            if new_start > start {
                start = new_start;
            } else {
                start = split_at;
            }
        } else {
            start = split_at;
        }
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }

    chunks.retain(|c| !c.is_empty());
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_no_split() {
        let text = "Hello world";
        let chunks = split_text(text, 1500, 200);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn empty_text() {
        let chunks = split_text("", 1500, 200);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn splits_on_paragraph_boundary() {
        let para1 = "A".repeat(800);
        let para2 = "B".repeat(800);
        let text = format!("{}\n\n{}", para1, para2);
        let chunks = split_text(&text, 1000, 100);
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
        assert!(chunks[0].contains(&"A".repeat(100)));
        assert!(chunks[chunks.len() - 1].contains(&"B".repeat(100)));
    }

    #[test]
    fn overlap_present_between_chunks() {
        // Unique identifiable segments separated by paragraphs
        let segments: Vec<String> = (0..15)
            .map(|i| format!("UniqueSegment{:03}", i))
            .collect();
        let text = segments.join("\n\n");
        let chunks = split_text(&text, 100, 40);
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
        // Last segment of chunk[i] must appear in chunk[i+1]
        for i in 0..chunks.len() - 1 {
            let last_seg = chunks[i].rsplit("\n\n").next().unwrap_or(&chunks[i]);
            assert!(
                chunks[i + 1].contains(last_seg),
                "overlap missing: last segment of chunk {} ('{}') not in chunk {}",
                i, last_seg, i + 1
            );
        }
    }

    #[test]
    fn falls_back_to_newline_split() {
        let lines: Vec<String> = (0..20)
            .map(|i| format!("Line {}: {}", i, "x".repeat(80)))
            .collect();
        let text = lines.join("\n");
        let chunks = split_text(&text, 500, 50);
        assert!(chunks.len() >= 2, "expected split on newlines");
    }

    #[test]
    fn falls_back_to_sentence_split() {
        let text = "First sentence. Second sentence. Third sentence. ".repeat(20);
        let chunks = split_text(&text, 200, 30);
        assert!(chunks.len() >= 2, "expected split on sentences");
    }

    #[test]
    fn hard_cut_when_no_boundaries() {
        let text = "a".repeat(3000);
        let chunks = split_text(&text, 1500, 200);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].len() <= 1500);
        assert!(chunks[1].len() <= 1500);
    }

    #[test]
    fn unicode_safe() {
        let text = "Привет мир. ".repeat(200);
        let chunks = split_text(&text, 500, 50);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(0));
        }
    }

    #[test]
    fn respects_max_chars() {
        let text = "Word. ".repeat(1000);
        let chunks = split_text(&text, 300, 50);
        for chunk in &chunks {
            assert!(chunk.len() <= 350,
                "chunk too large: {} chars", chunk.len());
        }
    }

    #[test]
    fn text_exactly_at_boundary() {
        let text = "a".repeat(1500);
        let chunks = split_text(&text, 1500, 200);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1500);
    }

    #[test]
    fn text_one_byte_over_boundary() {
        let text = "a".repeat(1501);
        let chunks = split_text(&text, 1500, 200);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn overlap_guard_clamps_to_half() {
        // overlap > max_chars/2 should be clamped
        let text = "A".repeat(100) + "\n\n" + &"B".repeat(100);
        let chunks = split_text(&text, 50, 9999);
        assert!(chunks.len() >= 2, "should still split despite huge overlap");
    }

    #[test]
    fn zero_overlap_no_shared_content() {
        let text = "AAA\n\nBBB\n\nCCC\n\nDDD";
        let chunks = split_text(text, 10, 0);
        assert!(chunks.len() >= 2);
        // With 0 overlap, adjacent chunks should not share segments
        for i in 0..chunks.len() - 1 {
            let last_seg = chunks[i].rsplit("\n\n").next().unwrap_or(&chunks[i]);
            if !last_seg.is_empty() {
                assert!(
                    !chunks[i + 1].starts_with(last_seg),
                    "chunk {} last seg '{}' overlaps into chunk {}", i, last_seg, i + 1
                );
            }
        }
    }

    #[test]
    fn many_small_paragraphs() {
        let text = (0..100).map(|i| format!("P{}", i)).collect::<Vec<_>>().join("\n\n");
        let chunks = split_text(&text, 50, 10);
        assert!(chunks.len() >= 5);
        // All content preserved
        let joined = chunks.join("");
        for i in 0..100 {
            assert!(joined.contains(&format!("P{}", i)), "missing P{}", i);
        }
    }

    #[test]
    fn mixed_separators() {
        // Text has paragraphs, newlines, and sentences
        let text = "Para one.\n\nPara two.\nLine three. Sentence four. End.";
        let chunks = split_text(text, 25, 5);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn whitespace_text_does_not_panic() {
        let text = "   \n\n   \n   ";
        let chunks = split_text(text, 5, 2);
        // Whitespace may be split on \n\n boundaries — verify no panic and no empty chunks
        for chunk in &chunks {
            assert!(!chunk.is_empty(), "empty chunk produced from whitespace input");
        }
    }

    #[test]
    fn single_very_long_line() {
        // No separators at all, just one continuous string with spaces
        let text = "word ".repeat(500);
        let chunks = split_text(&text, 100, 20);
        assert!(chunks.len() >= 10);
        for chunk in &chunks {
            assert!(chunk.len() <= 120, "chunk too large: {}", chunk.len());
        }
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn default_constants_reasonable() {
        assert!(DEFAULT_CHUNK_SIZE > 0);
        assert!(DEFAULT_CHUNK_OVERLAP < DEFAULT_CHUNK_SIZE);
        assert!(DEFAULT_CHUNK_OVERLAP > 0);
    }
}
