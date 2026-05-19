//! Syntax layer for MCDPL.
//!
//! The parser is intentionally lightweight at this foundation stage. It provides
//! stable tokenization, document-kind detection, statement-level structure,
//! spans, and recovery diagnostics that later CST/AST work can extend without
//! changing public callers.

use crate::{Diagnostic, Severity, SourceId, TextRange, TextSpan};

/// Top-level MCDPL document kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// `mcdp { ... }`
    Mcdp,
    /// `dp { ... }`
    Dp,
    /// `catalog { ... }`
    Catalog,
    /// `choose (...)`
    Choose,
    /// `intersection (...)`
    Intersection,
    /// `interface { ... }`
    Interface,
    /// `poset { ... }`
    Poset,
    /// `template [...] mcdp { ... }`
    Template,
    /// `specialize [...] ...`
    Specialize,
}

/// Token kind emitted by the lexer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// Language keyword.
    Keyword,
    /// Identifier or symbolic name.
    Ident,
    /// Number literal without attached unit.
    Number,
    /// Quoted string.
    String,
    /// Line comment.
    Comment,
    /// Horizontal whitespace.
    Whitespace,
    /// Newline.
    Newline,
    /// Operator or relation glyph.
    Operator,
    /// Punctuation.
    Punctuation,
    /// Unknown single character.
    Unknown,
}

/// Statement category recovered from a document body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementKind {
    /// Functionality declaration, for example `provides capacity [Wh]`.
    Provides,
    /// Requirement declaration, for example `requires mass [kg]`.
    Requires,
    /// Sub-problem instance declaration.
    Instance,
    /// Formula or constant assignment.
    Assignment,
    /// Feasibility/refinement/order relation.
    Constraint,
    /// External implementation binding, such as a YAML catalog resource.
    ImplementedBy,
    /// Interface implementation declaration.
    Implements,
    /// Library/model import.
    Import,
    /// Inline catalog implementation row.
    CatalogRecord,
    /// Recognized source line that is not yet semantically classified.
    BareExpression,
}

/// Parsed syntax document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxDocument {
    /// Detected document kind.
    pub kind: DocumentKind,
    /// Source range covered by the document.
    pub range: TextRange,
    /// Top-level body recovered for this document.
    pub body: SyntaxBody,
}

/// Top-level body shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyntaxBody {
    /// `{ ... }` body split into top-level statements.
    Braced {
        /// Statements recovered from the body.
        statements: Vec<Statement>,
    },
    /// `( ... )` body split into comma-separated entries.
    Parenthesized {
        /// Entries recovered from the body.
        entries: Vec<SyntaxEntry>,
    },
    /// `template [...] mcdp { ... }` body.
    Template {
        /// Template parameters.
        parameters: Vec<SyntaxEntry>,
        /// MCDP statements in the template body.
        statements: Vec<Statement>,
    },
    /// `specialize [...] target` body.
    Specialize {
        /// Specialization bindings.
        parameters: Vec<SyntaxEntry>,
        /// Target expression after the binding list.
        target: Option<SyntaxEntry>,
    },
    /// Body could not be recovered.
    Empty,
}

impl SyntaxBody {
    /// Returns recovered statements for body shapes that contain statements.
    #[must_use]
    pub fn statements(&self) -> &[Statement] {
        match self {
            Self::Braced { statements } | Self::Template { statements, .. } => statements,
            Self::Parenthesized { .. } | Self::Specialize { .. } | Self::Empty => &[],
        }
    }
}

/// Source segment recovered from comma-separated syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxEntry {
    /// Entry text with surrounding trivia removed.
    pub text: String,
    /// Entry byte range.
    pub range: TextRange,
}

/// Statement recovered from a braced document body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Statement {
    /// Coarse category used by compiler/lowering passes.
    pub kind: StatementKind,
    /// Statement text with surrounding trivia removed.
    pub text: String,
    /// Statement byte range.
    pub range: TextRange,
}

/// Source token with byte range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    /// Token kind.
    pub kind: TokenKind,
    /// Token text.
    pub text: String,
    /// Byte range in the original document.
    pub range: TextRange,
}

impl Token {
    fn new(kind: TokenKind, text: &str, start: usize, end: usize) -> Self {
        Self {
            kind,
            text: text[start..end].to_owned(),
            range: TextRange::new(start, end),
        }
    }
}

/// Parsed document shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedDocument {
    /// Detected top-level document kind.
    pub kind: Option<DocumentKind>,
    /// Recovered top-level syntax tree.
    pub syntax: Option<SyntaxDocument>,
    /// Full token stream, including trivia.
    pub tokens: Vec<Token>,
    /// Syntax diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedDocument {
    /// Returns true if parsing produced blocking diagnostics.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Parses a source document enough to classify and diagnose it.
#[must_use]
pub fn parse_document(source_id: SourceId, source: &str) -> ParsedDocument {
    let tokens = lex(source);
    let kind = detect_document_kind(&tokens);
    let mut diagnostics = Vec::new();
    check_balanced_delimiters(&source_id, &tokens, &mut diagnostics);

    if kind.is_none() {
        let span = tokens
            .iter()
            .find(|token| !is_trivia(token.kind))
            .map(|token| TextSpan::new(source_id.clone(), token.range));
        let mut diagnostic = Diagnostic::error(
            "syntax.unknown-document-kind",
            "expected a top-level MCDPL document kind",
        )
        .with_help("Start the file with mcdp, dp, catalog, choose, intersection, interface, poset, template, or specialize.");
        if let Some(span) = span {
            diagnostic = diagnostic.with_span(span);
        }
        diagnostics.push(diagnostic);
    }

    let syntax = kind.map(|document_kind| {
        parse_syntax_document(&source_id, &tokens, document_kind, &mut diagnostics)
    });

    ParsedDocument {
        kind,
        syntax,
        tokens,
        diagnostics,
    }
}

/// Tokenizes MCDPL source text.
#[must_use]
pub fn lex(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let kind;
        let mut end = start + ch.len_utf8();

        if ch == '\n' {
            kind = TokenKind::Newline;
        } else if ch.is_whitespace() {
            kind = TokenKind::Whitespace;
            consume_while(&mut chars, &mut end, |next| {
                next.is_whitespace() && next != '\n'
            });
        } else if ch == '#' {
            kind = TokenKind::Comment;
            consume_while(&mut chars, &mut end, |next| next != '\n');
        } else if ch == '"' || ch == '\'' {
            kind = TokenKind::String;
            consume_string(source, &mut chars, &mut end, ch);
        } else if ch.is_ascii_digit() {
            kind = TokenKind::Number;
            consume_while(&mut chars, &mut end, is_number_continue);
        } else if is_ident_start(ch) {
            consume_while(&mut chars, &mut end, is_ident_continue);
            let text = &source[start..end];
            kind = if is_keyword(text) {
                TokenKind::Keyword
            } else {
                TokenKind::Ident
            };
        } else if is_operator_char(ch) {
            kind = TokenKind::Operator;
            consume_while(&mut chars, &mut end, is_operator_char);
        } else if is_punctuation(ch) {
            kind = TokenKind::Punctuation;
        } else {
            kind = TokenKind::Unknown;
        }

        tokens.push(Token::new(kind, source, start, end));
    }

    tokens
}

fn consume_while(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    end: &mut usize,
    predicate: impl Fn(char) -> bool,
) {
    while let Some((next_index, next)) = chars.peek().copied() {
        if !predicate(next) {
            break;
        }
        *end = next_index + next.len_utf8();
        chars.next();
    }
}

fn consume_string(
    source: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    end: &mut usize,
    quote: char,
) {
    let mut previous_was_escape = false;
    for (next_index, next) in chars.by_ref() {
        *end = next_index + next.len_utf8();
        if previous_was_escape {
            previous_was_escape = false;
            continue;
        }
        if next == '\\' {
            previous_was_escape = true;
            continue;
        }
        if next == quote {
            break;
        }
    }

    if *end > source.len() {
        *end = source.len();
    }
}

fn detect_document_kind(tokens: &[Token]) -> Option<DocumentKind> {
    let first = tokens.iter().find(|token| !is_trivia(token.kind))?;
    match first.text.as_str() {
        "mcdp" => Some(DocumentKind::Mcdp),
        "dp" => Some(DocumentKind::Dp),
        "catalog" => Some(DocumentKind::Catalog),
        "choose" => Some(DocumentKind::Choose),
        "intersection" => Some(DocumentKind::Intersection),
        "interface" => Some(DocumentKind::Interface),
        "poset" => Some(DocumentKind::Poset),
        "template" => Some(DocumentKind::Template),
        "specialize" => Some(DocumentKind::Specialize),
        _ => None,
    }
}

fn parse_syntax_document(
    source_id: &SourceId,
    tokens: &[Token],
    kind: DocumentKind,
    diagnostics: &mut Vec<Diagnostic>,
) -> SyntaxDocument {
    let first_index = first_non_trivia_index(tokens).unwrap_or(0);
    let range = document_range(tokens, first_index);
    let body = match kind {
        DocumentKind::Mcdp
        | DocumentKind::Dp
        | DocumentKind::Catalog
        | DocumentKind::Interface
        | DocumentKind::Poset => parse_braced_body(source_id, tokens, first_index, diagnostics),
        DocumentKind::Choose | DocumentKind::Intersection => {
            parse_parenthesized_body(source_id, tokens, first_index, diagnostics)
        }
        DocumentKind::Template => parse_template_body(source_id, tokens, first_index, diagnostics),
        DocumentKind::Specialize => {
            parse_specialize_body(source_id, tokens, first_index, diagnostics)
        }
    };

    SyntaxDocument { kind, range, body }
}

fn parse_braced_body(
    source_id: &SourceId,
    tokens: &[Token],
    first_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> SyntaxBody {
    let Some(open_index) = find_next_text(tokens, first_index + 1, "{") else {
        diagnostics.push(expected_body_diagnostic(
            source_id,
            &tokens[first_index],
            "expected `{ ... }` after the document kind",
        ));
        return SyntaxBody::Empty;
    };

    let close_index = match find_matching_forward(tokens, open_index, "{", "}") {
        Some(index) => index,
        None => tokens.len(),
    };
    SyntaxBody::Braced {
        statements: split_statements(tokens, open_index + 1, close_index),
    }
}

fn parse_parenthesized_body(
    source_id: &SourceId,
    tokens: &[Token],
    first_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> SyntaxBody {
    let Some(open_index) = find_next_text(tokens, first_index + 1, "(") else {
        diagnostics.push(expected_body_diagnostic(
            source_id,
            &tokens[first_index],
            "expected `( ... )` after the document kind",
        ));
        return SyntaxBody::Empty;
    };

    let close_index = match find_matching_forward(tokens, open_index, "(", ")") {
        Some(index) => index,
        None => tokens.len(),
    };
    SyntaxBody::Parenthesized {
        entries: split_entries(tokens, open_index + 1, close_index),
    }
}

fn parse_template_body(
    source_id: &SourceId,
    tokens: &[Token],
    first_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> SyntaxBody {
    let Some(open_index) = find_next_text(tokens, first_index + 1, "[") else {
        diagnostics.push(expected_body_diagnostic(
            source_id,
            &tokens[first_index],
            "expected template parameter list after `template`",
        ));
        return SyntaxBody::Empty;
    };

    let close_index = match find_matching_forward(tokens, open_index, "[", "]") {
        Some(index) => index,
        None => tokens.len(),
    };
    let parameters = split_entries(tokens, open_index + 1, close_index);

    let Some(model_index) = find_next_text(tokens, close_index, "mcdp") else {
        diagnostics.push(expected_body_diagnostic(
            source_id,
            &tokens[first_index],
            "expected `mcdp { ... }` after template parameters",
        ));
        return SyntaxBody::Template {
            parameters,
            statements: Vec::new(),
        };
    };

    match parse_braced_body(source_id, tokens, model_index, diagnostics) {
        SyntaxBody::Braced { statements } => SyntaxBody::Template {
            parameters,
            statements,
        },
        _ => SyntaxBody::Template {
            parameters,
            statements: Vec::new(),
        },
    }
}

fn parse_specialize_body(
    source_id: &SourceId,
    tokens: &[Token],
    first_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> SyntaxBody {
    let Some(open_index) = find_next_text(tokens, first_index + 1, "[") else {
        diagnostics.push(expected_body_diagnostic(
            source_id,
            &tokens[first_index],
            "expected specialization binding list after `specialize`",
        ));
        return SyntaxBody::Empty;
    };

    let close_index = match find_matching_forward(tokens, open_index, "[", "]") {
        Some(index) => index,
        None => tokens.len(),
    };
    SyntaxBody::Specialize {
        parameters: split_entries(tokens, open_index + 1, close_index),
        target: segment_entry(tokens, close_index + 1, tokens.len()),
    }
}

fn document_range(tokens: &[Token], first_index: usize) -> TextRange {
    let start = tokens.get(first_index).map_or(0, |token| token.range.start);
    let end = tokens
        .iter()
        .rev()
        .find(|token| !is_trivia(token.kind))
        .map_or(start, |token| token.range.end);
    TextRange::new(start, end)
}

fn first_non_trivia_index(tokens: &[Token]) -> Option<usize> {
    tokens.iter().position(|token| !is_trivia(token.kind))
}

fn find_next_text(tokens: &[Token], start: usize, text: &str) -> Option<usize> {
    (start..tokens.len()).find(|index| tokens[*index].text == text)
}

fn find_matching_forward(
    tokens: &[Token],
    open_index: usize,
    open_text: &str,
    close_text: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.kind == TokenKind::Comment {
            continue;
        }
        if token.text == open_text {
            depth += 1;
            continue;
        }
        if token.text == close_text {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn split_statements(tokens: &[Token], start: usize, end: usize) -> Vec<Statement> {
    let mut statements = Vec::new();
    let mut segment_start = start;
    let mut depth = DelimiterDepth::default();

    for index in start..end {
        let token = &tokens[index];
        depth.observe(token);
        if depth.is_zero() && (token.kind == TokenKind::Newline || token.text.as_str() == ";") {
            if let Some(statement) = build_statement(tokens, segment_start, index) {
                statements.push(statement);
            }
            segment_start = index + 1;
        }
    }

    if let Some(statement) = build_statement(tokens, segment_start, end) {
        statements.push(statement);
    }

    statements
}

fn split_entries(tokens: &[Token], start: usize, end: usize) -> Vec<SyntaxEntry> {
    let mut entries = Vec::new();
    let mut segment_start = start;
    let mut depth = DelimiterDepth::default();

    for index in start..end {
        let token = &tokens[index];
        depth.observe(token);
        if depth.is_zero() && token.text.as_str() == "," {
            if let Some(entry) = segment_entry(tokens, segment_start, index) {
                entries.push(entry);
            }
            segment_start = index + 1;
        }
    }

    if let Some(entry) = segment_entry(tokens, segment_start, end) {
        entries.push(entry);
    }

    entries
}

fn build_statement(tokens: &[Token], start: usize, end: usize) -> Option<Statement> {
    let first = first_non_trivia_in_range(tokens, start, end)?;
    let last = last_non_trivia_in_range(tokens, start, end)?;
    let text = tokens_text(tokens, first, last + 1);
    let kind = classify_statement(&tokens[first..=last], &text);
    Some(Statement {
        kind,
        text,
        range: TextRange::new(tokens[first].range.start, tokens[last].range.end),
    })
}

fn segment_entry(tokens: &[Token], start: usize, end: usize) -> Option<SyntaxEntry> {
    let first = first_non_trivia_in_range(tokens, start, end)?;
    let last = last_non_trivia_in_range(tokens, start, end)?;
    Some(SyntaxEntry {
        text: tokens_text(tokens, first, last + 1),
        range: TextRange::new(tokens[first].range.start, tokens[last].range.end),
    })
}

fn first_non_trivia_in_range(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    (start..end).find(|index| !is_trivia(tokens[*index].kind))
}

fn last_non_trivia_in_range(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    (start..end)
        .rev()
        .find(|index| !is_trivia(tokens[*index].kind))
}

fn tokens_text(tokens: &[Token], start: usize, end: usize) -> String {
    tokens[start..end]
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn classify_statement(tokens: &[Token], text: &str) -> StatementKind {
    let significant: Vec<&str> = tokens
        .iter()
        .filter(|token| !is_trivia(token.kind))
        .map(|token| token.text.as_str())
        .collect();
    let Some(first) = significant.first().copied() else {
        return StatementKind::BareExpression;
    };

    if first == "provides" {
        return StatementKind::Provides;
    }
    if first == "requires" {
        return StatementKind::Requires;
    }
    if first == "from" || first == "import" {
        return StatementKind::Import;
    }
    if starts_implemented_by(&significant) {
        return StatementKind::ImplementedBy;
    }
    if first == "implements" {
        return StatementKind::Implements;
    }
    if significant.contains(&"instance") {
        return StatementKind::Instance;
    }
    if contains_catalog_arrow(text) {
        return StatementKind::CatalogRecord;
    }
    if significant.iter().any(|token| is_relation_operator(token)) {
        return StatementKind::Constraint;
    }
    if significant.contains(&"=") {
        return StatementKind::Assignment;
    }

    StatementKind::BareExpression
}

fn starts_implemented_by(tokens: &[&str]) -> bool {
    tokens.first() == Some(&"implemented-by") || matches!(tokens, ["implemented", "-", "by", ..])
}

fn contains_catalog_arrow(text: &str) -> bool {
    text.contains("<-|")
        || text.contains("|->")
        || text.contains("<--|")
        || text.contains("|-->")
        || text.contains('↤')
        || text.contains('↦')
        || text.contains('⟷')
}

fn is_relation_operator(text: &str) -> bool {
    matches!(text, "<=" | ">=" | "≤" | "≥" | "⪯" | "⪰")
}

fn expected_body_diagnostic(source_id: &SourceId, token: &Token, message: &str) -> Diagnostic {
    Diagnostic::error("syntax.expected-body", message)
        .with_span(TextSpan::new(source_id.clone(), token.range))
}

fn check_balanced_delimiters(
    source_id: &SourceId,
    tokens: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut stack: Vec<(&str, TextRange)> = Vec::new();

    for token in tokens {
        if token.kind == TokenKind::Comment {
            continue;
        }
        match token.text.as_str() {
            "{" | "(" | "[" | "⟨" => stack.push((token.text.as_str(), token.range)),
            "}" | ")" | "]" | "⟩" => {
                let expected_open = expected_open_delimiter(token.text.as_str());
                match stack.pop() {
                    Some((open, _)) if open == expected_open => {}
                    Some((open, open_range)) => diagnostics.push(
                        Diagnostic::error(
                            "syntax.unbalanced-delimiter",
                            format!("closing delimiter `{}` does not match `{open}`", token.text),
                        )
                        .with_span(TextSpan::new(source_id.clone(), open_range)),
                    ),
                    None => diagnostics.push(
                        Diagnostic::error(
                            "syntax.unbalanced-delimiter",
                            format!("unexpected closing delimiter `{}`", token.text),
                        )
                        .with_span(TextSpan::new(source_id.clone(), token.range)),
                    ),
                }
            }
            _ => {}
        }
    }

    for (open, range) in stack {
        diagnostics.push(
            Diagnostic::error(
                "syntax.unbalanced-delimiter",
                format!("unclosed delimiter `{open}`"),
            )
            .with_span(TextSpan::new(source_id.clone(), range)),
        );
    }
}

fn expected_open_delimiter(close: &str) -> &'static str {
    match close {
        "}" => "{",
        ")" => "(",
        "]" => "[",
        "⟩" => "⟨",
        _ => "",
    }
}

#[derive(Default)]
struct DelimiterDepth {
    braces: usize,
    parentheses: usize,
    brackets: usize,
    angles: usize,
}

impl DelimiterDepth {
    fn observe(&mut self, token: &Token) {
        if token.kind == TokenKind::Comment {
            return;
        }

        match token.text.as_str() {
            "{" => self.braces += 1,
            "}" => self.braces = self.braces.saturating_sub(1),
            "(" => self.parentheses += 1,
            ")" => self.parentheses = self.parentheses.saturating_sub(1),
            "[" => self.brackets += 1,
            "]" => self.brackets = self.brackets.saturating_sub(1),
            "⟨" => self.angles += 1,
            "⟩" => self.angles = self.angles.saturating_sub(1),
            _ => {}
        }
    }

    fn is_zero(&self) -> bool {
        self.braces == 0 && self.parentheses == 0 && self.brackets == 0 && self.angles == 0
    }
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
    )
}

fn is_keyword(text: &str) -> bool {
    matches!(
        text,
        "mcdp"
            | "dp"
            | "catalog"
            | "choose"
            | "intersection"
            | "interface"
            | "poset"
            | "template"
            | "specialize"
            | "provides"
            | "requires"
            | "provided"
            | "required"
            | "instance"
            | "implemented-by"
            | "implements"
            | "resource"
            | "from"
            | "library"
            | "import"
            | "model"
            | "sub"
            | "constant"
            | "sum"
            | "by"
    )
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic() || ch.is_numeric()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric() || ch.is_numeric()
}

fn is_number_continue(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '.' | '_')
}

fn is_operator_char(ch: char) -> bool {
    matches!(
        ch,
        '<' | '>'
            | '='
            | '≤'
            | '≥'
            | '⪯'
            | '⪰'
            | '←'
            | '→'
            | '↤'
            | '↦'
            | '⟷'
            | '-'
            | '|'
            | '*'
            | '/'
            | '+'
            | '^'
            | '·'
    )
}

fn is_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '{' | '}' | '(' | ')' | '[' | ']' | ',' | ':' | '.' | ';' | '`' | '⟨' | '⟩'
    )
}

#[cfg(test)]
mod tests {
    use super::{DocumentKind, StatementKind, SyntaxBody, TokenKind, lex, parse_document};
    use crate::SourceId;

    #[test]
    fn detects_mcdp_document() {
        let parsed = parse_document(
            SourceId::new("battery.mcdp"),
            "mcdp { provides capacity [J] }",
        );

        assert_eq!(parsed.kind, Some(DocumentKind::Mcdp));
        assert!(!parsed.has_errors());
        assert_eq!(
            parsed
                .syntax
                .as_ref()
                .map(|syntax| syntax.body.statements().len()),
            Some(1)
        );
    }

    #[test]
    fn keeps_unicode_operators() {
        let tokens = lex("provided capacity ≤ 500 J");
        let operators: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Operator)
            .map(|token| token.text.as_str())
            .collect();

        assert_eq!(operators, vec!["≤"]);
    }

    #[test]
    fn reports_unknown_document_kind() {
        let parsed = parse_document(SourceId::new("bad.mcdp"), "unknown { }");

        assert!(parsed.has_errors());
        assert_eq!(parsed.diagnostics[0].code, "syntax.unknown-document-kind");
    }

    #[test]
    fn classifies_core_statement_shapes() {
        let parsed = parse_document(
            SourceId::new("rover.mcdp"),
            "\
mcdp {
  provides velocity [m/s]
  requires cost [USD]
  battery_dp = instance `battery
  provided velocity <= velocity provided by battery_dp
  mass = 10 kg
}
",
        );

        let kinds = statement_kinds(&parsed);

        assert_eq!(
            kinds,
            vec![
                StatementKind::Provides,
                StatementKind::Requires,
                StatementKind::Instance,
                StatementKind::Constraint,
                StatementKind::Assignment,
            ]
        );
    }

    #[test]
    fn classifies_direct_import_statement() {
        let parsed = parse_document(
            SourceId::new("system.mcdp"),
            "\
mcdp {
  import model `battery
}
",
        );

        let kinds = statement_kinds(&parsed);

        assert_eq!(kinds, vec![StatementKind::Import]);
    }

    #[test]
    fn parses_choose_entries() {
        let parsed = parse_document(
            SourceId::new("Batteries.mcdp"),
            "choose (NiMH: `Battery_NiMH, LFP: `Battery_LFP)",
        );

        let entry_count = match parsed.syntax.as_ref().map(|syntax| &syntax.body) {
            Some(SyntaxBody::Parenthesized { entries }) => entries.len(),
            _ => 0,
        };

        assert_eq!(entry_count, 2);
        assert!(!parsed.has_errors());
    }

    #[test]
    fn parses_intersection_entries() {
        let parsed = parse_document(
            SourceId::new("Both.mcdp"),
            "intersection (A: `Battery_A, B: `Battery_B)",
        );

        let entry_count = match parsed.syntax.as_ref().map(|syntax| &syntax.body) {
            Some(SyntaxBody::Parenthesized { entries }) => entries.len(),
            _ => 0,
        };

        assert_eq!(parsed.kind, Some(DocumentKind::Intersection));
        assert_eq!(entry_count, 2);
        assert!(!parsed.has_errors());
    }

    #[test]
    fn keeps_multiline_product_declaration_as_one_statement() {
        let parsed = parse_document(
            SourceId::new("catalog_planner.mcdp"),
            "\
dp {
  requires dyn_prop [product(v: m/s,
                             max_lateral_a: m/s²,
                             path_type: `path_type)]
  implemented-by yaml resource(\"catalog.yaml\")
}
",
        );

        let kinds = statement_kinds(&parsed);

        assert_eq!(
            kinds,
            vec![StatementKind::Requires, StatementKind::ImplementedBy]
        );
        assert!(!parsed.has_errors());
    }

    #[test]
    fn classifies_catalog_record() {
        let parsed = parse_document(
            SourceId::new("catalog.mcdp"),
            "catalog { ⟨5 m/s, 1 m⟩ <-| imp1 |-> 10 kg, 4000 USD }",
        );

        assert_eq!(statement_kinds(&parsed), vec![StatementKind::CatalogRecord]);
        assert!(!parsed.has_errors());
    }

    #[test]
    fn reports_unbalanced_delimiters() {
        let parsed = parse_document(SourceId::new("bad.mcdp"), "mcdp { provides x [kg] ");

        assert!(parsed.has_errors());
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "syntax.unbalanced-delimiter")
        );
    }

    fn statement_kinds(parsed: &super::ParsedDocument) -> Vec<StatementKind> {
        match parsed.syntax.as_ref().map(|syntax| &syntax.body) {
            Some(SyntaxBody::Braced { statements })
            | Some(SyntaxBody::Template { statements, .. }) => {
                statements.iter().map(|statement| statement.kind).collect()
            }
            _ => Vec::new(),
        }
    }
}
