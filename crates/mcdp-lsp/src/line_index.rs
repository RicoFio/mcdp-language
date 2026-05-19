//! UTF-8 byte and LSP UTF-16 position conversion utilities.

use mcdp_language::TextRange;
use tower_lsp::lsp_types::{Position, Range};

#[derive(Debug)]
pub(crate) struct LineIndex<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (index, ch) in source.char_indices() {
            if ch == '\n' {
                line_starts.push(index + ch.len_utf8());
            }
        }

        Self {
            source,
            line_starts,
        }
    }

    pub(crate) fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start), self.position(range.end))
    }

    pub(crate) fn position(&self, byte_offset: usize) -> Position {
        let byte_offset = self.clamp_to_char_boundary(byte_offset.min(self.source.len()));
        let line = self.line_for_offset(byte_offset);
        let line_start = self.line_starts[line];
        let character = self.source[line_start..byte_offset].encode_utf16().count();

        Position::new(saturating_u32(line), saturating_u32(character))
    }

    pub(crate) fn offset(&self, position: Position) -> usize {
        let requested_line = position.line as usize;
        let Some(line_start) = self.line_starts.get(requested_line).copied() else {
            return self.source.len();
        };
        let line_end = self.line_end(requested_line);
        let requested_character = position.character as usize;
        let mut utf16_character = 0;

        for (relative_offset, ch) in self.source[line_start..line_end].char_indices() {
            if utf16_character >= requested_character {
                return line_start + relative_offset;
            }
            let width = ch.len_utf16();
            if utf16_character + width > requested_character {
                return line_start + relative_offset;
            }
            utf16_character += width;
        }

        line_end
    }

    pub(crate) fn token_segments(&self, range: TextRange) -> Vec<TokenSegment> {
        let start = self.clamp_to_char_boundary(range.start.min(self.source.len()));
        let end = self.clamp_to_char_boundary(range.end.min(self.source.len()));
        if start >= end {
            return Vec::new();
        }

        let start_line = self.line_for_offset(start);
        let end_line = self.line_for_offset(end);
        let mut segments = Vec::new();

        for line in start_line..=end_line {
            let line_start = self.line_starts[line];
            let line_end = self.line_end(line);
            let segment_start = start.max(line_start);
            let segment_end = end.min(line_end);
            if segment_start >= segment_end {
                continue;
            }

            let start_position = self.position(segment_start);
            let end_position = self.position(segment_end);
            segments.push(TokenSegment {
                line: start_position.line,
                start: start_position.character,
                length: end_position
                    .character
                    .saturating_sub(start_position.character),
            });
        }

        segments
    }

    fn clamp_to_char_boundary(&self, mut byte_offset: usize) -> usize {
        while !self.source.is_char_boundary(byte_offset) {
            byte_offset = byte_offset.saturating_sub(1);
        }
        byte_offset
    }

    fn line_for_offset(&self, byte_offset: usize) -> usize {
        self.line_starts
            .partition_point(|line_start| *line_start <= byte_offset)
            .saturating_sub(1)
    }

    fn line_end(&self, line: usize) -> usize {
        let Some(next_line_start) = self.line_starts.get(line + 1).copied() else {
            return self.source.len();
        };
        let mut line_end = next_line_start.saturating_sub(1);
        if line_end > self.line_starts[line] && self.source.as_bytes()[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        line_end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TokenSegment {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) length: u32,
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ascii_offsets_to_lsp_positions() {
        let source = "mcdp {\n  provides speed [Nat]\n}\n";
        let line_index = LineIndex::new(source);

        assert_eq!(
            line_index.position(offset_of(source, "provides")),
            Position::new(1, 2)
        );
        assert_eq!(line_index.position(source.len()), Position::new(3, 0));
    }

    #[test]
    fn counts_utf16_columns_for_unicode_source() {
        let source = "mcdp {\n  provides speed [m/s²]\n  provided speed ≤ 10\n}\n";
        let line_index = LineIndex::new(source);
        let squared_offset = offset_of(source, "²");
        let relation_offset = offset_of(source, "≤");

        assert_eq!(line_index.position(squared_offset), Position::new(1, 21));
        assert_eq!(
            line_index.position(squared_offset + "²".len()),
            Position::new(1, 22)
        );
        assert_eq!(line_index.position(relation_offset), Position::new(2, 17));
        assert_eq!(
            line_index.position(relation_offset + "≤".len()),
            Position::new(2, 18)
        );
    }

    #[test]
    fn clamps_non_char_boundary_offsets() {
        let source = "mcdp { α }";
        let line_index = LineIndex::new(source);
        let alpha_offset = offset_of(source, "α");

        assert_eq!(
            line_index.position(alpha_offset + 1),
            line_index.position(alpha_offset)
        );
    }

    #[test]
    fn maps_lsp_positions_back_to_byte_offsets() {
        let source = "mcdp {\n  provides speed [m/s²]\n}\n";
        let line_index = LineIndex::new(source);

        assert_eq!(
            line_index.offset(Position::new(1, 2)),
            offset_of(source, "provides")
        );
        assert_eq!(
            line_index.offset(Position::new(1, 22)),
            offset_of(source, "]")
        );
        assert_eq!(line_index.offset(Position::new(99, 0)), source.len());
    }

    fn offset_of(source: &str, needle: &str) -> usize {
        match source.find(needle) {
            Some(offset) => offset,
            None => panic!("missing `{needle}` in test source"),
        }
    }
}
