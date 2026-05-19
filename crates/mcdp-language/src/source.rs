//! Source identity and span types shared by diagnostics and syntax trees.

use std::fmt;

/// Stable identifier for a source file or virtual document.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(String);

impl SourceId {
    /// Creates a source identifier from a path, URI, or virtual name.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the source identifier as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Half-open byte range in a UTF-8 source document.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextRange {
    /// First byte included in the range.
    pub start: usize,
    /// First byte after the range.
    pub end: usize,
}

impl TextRange {
    /// Creates a new half-open range.
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Self { start, end }
    }

    /// Returns the range length in bytes.
    #[must_use]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns true when the range contains no bytes.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Source-qualified text span.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TextSpan {
    /// Source file or virtual document.
    pub source: SourceId,
    /// Byte range in the source.
    pub range: TextRange,
}

impl TextSpan {
    /// Creates a new source-qualified span.
    #[must_use]
    pub fn new(source: SourceId, range: TextRange) -> Self {
        Self { source, range }
    }
}
