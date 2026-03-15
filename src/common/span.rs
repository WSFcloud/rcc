use chumsky::span::SimpleSpan;
use std::ops::Range;

/// Span semantics used across lexer/parser/AST:
/// - Offsets are UTF-8 byte indices into the original source buffer.
/// - Ranges are half-open: `[start, end)`.
/// - `start`/`end` are stable diagnostic anchors; they are not character indices.
/// - Composite syntax nodes should cover their full surface form
/// - including delimiters and terminators when present in grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

impl SourceSpan {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "invalid span: start > end");
        Self { start, end }
    }

    #[cfg(test)]
    pub fn dummy() -> Self {
        Self { start: 0, end: 0 }
    }

    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn join(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }

    /// Slice source text by this span.
    ///
    /// Returns `None` when the span is out of bounds or not on UTF-8 boundaries.
    pub fn text<'a>(&self, source: &'a str) -> Option<&'a str> {
        source.get(self.start..self.end)
    }
}

// Bridge from chumsky spans.
// Current parser uses `SimpleSpan<usize>` with default context `()`.
// We keep generic `C` for future context expansion.
impl<C> From<SimpleSpan<usize, C>> for SourceSpan {
    fn from(span: SimpleSpan<usize, C>) -> Self {
        Self::new(span.start, span.end)
    }
}

// Bridge to/from range, used by ariadne labels.
impl From<SourceSpan> for Range<usize> {
    fn from(span: SourceSpan) -> Self {
        span.start..span.end
    }
}

impl From<Range<usize>> for SourceSpan {
    fn from(range: Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_and_len_work() {
        let span = SourceSpan::new(3, 8);
        assert!(span.contains(3));
        assert!(span.contains(7));
        assert!(!span.contains(8));
        assert_eq!(span.len(), 5);
        assert!(!span.is_empty());
    }

    #[test]
    fn join_creates_union() {
        let lhs = SourceSpan::new(4, 10);
        let rhs = SourceSpan::new(2, 6);
        assert_eq!(lhs.join(rhs), SourceSpan::new(2, 10));
    }

    #[test]
    fn converts_from_simple_span() {
        let simple: SimpleSpan<usize> = (5..12).into();
        let span: SourceSpan = simple.into();
        assert_eq!(span, SourceSpan::new(5, 12));
    }

    #[test]
    fn converts_to_and_from_range() {
        let original = SourceSpan::new(9, 15);
        let range: Range<usize> = original.into();
        assert_eq!(range, 9..15);

        let roundtrip: SourceSpan = range.into();
        assert_eq!(roundtrip, SourceSpan::new(9, 15));
    }

    #[test]
    fn dummy_is_zero_width() {
        let span = SourceSpan::dummy();
        assert_eq!(span.start, 0);
        assert_eq!(span.end, 0);
        assert_eq!(span.len(), 0);
        assert!(span.is_empty());
    }

    #[test]
    fn text_slices_source() {
        let src = "int value = 42;";
        let span = SourceSpan::new(4, 9);
        assert_eq!(span.text(src), Some("value"));
    }

    #[test]
    fn text_returns_none_when_out_of_bounds() {
        let src = "abc";
        let span = SourceSpan::new(0, 10);
        assert_eq!(span.text(src), None);
    }

    #[test]
    fn text_returns_none_for_non_utf8_boundary() {
        let src = "a\u{4F60}b";
        let span = SourceSpan::new(1, 2);
        assert_eq!(span.text(src), None);
    }
}
