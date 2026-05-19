//! User-facing diagnostics emitted by all frontend and solver stages.

use crate::TextSpan;

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    /// Compilation or solving cannot continue correctly.
    Error,
    /// Suspicious but not blocking.
    Warning,
    /// Informational note for tooling.
    Info,
}

/// A source-aware diagnostic with stable machine-readable code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// Stable diagnostic code, for example `syntax.unknown-document-kind`.
    pub code: String,
    /// Severity level.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Optional actionable help text.
    pub help: Option<String>,
    /// Optional source span.
    pub span: Option<TextSpan>,
}

impl Diagnostic {
    /// Creates an error diagnostic.
    #[must_use]
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span: None,
        }
    }

    /// Creates a warning diagnostic.
    #[must_use]
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Warning,
            message: message.into(),
            help: None,
            span: None,
        }
    }

    /// Attaches help text.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Attaches a source span.
    #[must_use]
    pub fn with_span(mut self, span: TextSpan) -> Self {
        self.span = Some(span);
        self
    }
}

/// Diagnostics returned by a checking pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckReport {
    /// Ordered diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckReport {
    /// Creates an empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a diagnostic to the report.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Extends the report with diagnostics from another stage.
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    /// Returns true if at least one error was emitted.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}
