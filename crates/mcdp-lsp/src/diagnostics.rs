//! Conversion from MCDP diagnostics to LSP diagnostics.

use mcdp_language::{
    Diagnostic as McdpDiagnostic, Severity, SourceId, lower_document, parse_document,
};
use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, NumberOrString, Position, Range,
};

use crate::line_index::LineIndex;
use crate::project_symbols::{ProjectDiagnostic, ProjectSymbolIndex};

pub(crate) fn syntax_diagnostics(source_id: &str, source: &str) -> Vec<LspDiagnostic> {
    let parsed = parse_document(SourceId::new(source_id), source);
    let line_index = LineIndex::new(source);
    parsed
        .diagnostics
        .iter()
        .map(|diagnostic| lsp_diagnostic(&line_index, diagnostic))
        .collect()
}

pub(crate) fn document_diagnostics(
    source_id: &str,
    source: &str,
    index: Option<&ProjectSymbolIndex>,
) -> Vec<LspDiagnostic> {
    let mut diagnostics = syntax_diagnostics(source_id, source);
    diagnostics.extend(compiler_diagnostics(source_id, source));
    if let Some(index) = index
        && let Ok(uri) = tower_lsp::lsp_types::Url::parse(source_id)
    {
        let line_index = LineIndex::new(source);
        diagnostics.extend(
            index
                .semantic_diagnostics(&uri)
                .iter()
                .map(|diagnostic| lsp_project_diagnostic(&line_index, diagnostic)),
        );
    }
    diagnostics
}

fn compiler_diagnostics(source_id: &str, source: &str) -> Vec<LspDiagnostic> {
    let source_id = SourceId::new(source_id);
    let parsed = parse_document(source_id.clone(), source);
    if parsed.has_errors() {
        return Vec::new();
    }

    let (_, diagnostics) = lower_document(source_id, &parsed);
    let line_index = LineIndex::new(source);
    diagnostics
        .iter()
        .map(|diagnostic| {
            let mut diagnostic = lsp_diagnostic(&line_index, diagnostic);
            diagnostic.source = Some("mcdp-compiler".to_owned());
            diagnostic
        })
        .collect()
}

fn lsp_diagnostic(line_index: &LineIndex<'_>, diagnostic: &McdpDiagnostic) -> LspDiagnostic {
    LspDiagnostic {
        range: diagnostic.span.as_ref().map_or_else(
            || Range::new(Position::new(0, 0), Position::new(0, 0)),
            |span| line_index.range(span.range),
        ),
        severity: Some(lsp_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.clone())),
        source: Some("mcdp-syntax".to_owned()),
        message: diagnostic_message(diagnostic),
        ..LspDiagnostic::default()
    }
}

fn lsp_project_diagnostic(
    line_index: &LineIndex<'_>,
    diagnostic: &ProjectDiagnostic,
) -> LspDiagnostic {
    LspDiagnostic {
        range: line_index.range(diagnostic.range),
        severity: Some(lsp_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.clone())),
        source: Some("mcdp-lsp".to_owned()),
        message: match &diagnostic.help {
            Some(help) => format!("{}\nhelp: {help}", diagnostic.message),
            None => diagnostic.message.clone(),
        },
        ..LspDiagnostic::default()
    }
}

fn diagnostic_message(diagnostic: &McdpDiagnostic) -> String {
    match &diagnostic.help {
        Some(help) => format!("{}\nhelp: {help}", diagnostic.message),
        None => diagnostic.message.clone(),
    }
}

fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

#[cfg(test)]
mod tests {
    use mcdp_language::{Diagnostic as McdpDiagnostic, SourceId, TextRange, TextSpan};

    use super::*;

    #[test]
    fn lsp_diagnostic_preserves_severity_code_and_help() {
        let source = "bad { }";
        let line_index = LineIndex::new(source);
        let diagnostic = McdpDiagnostic::error(
            "syntax.unknown-document-kind",
            "expected a top-level MCDPL document kind",
        )
        .with_help("Start the file with mcdp.")
        .with_span(TextSpan::new(
            SourceId::new("test.mcdp"),
            TextRange::new(0, 3),
        ));

        let converted = lsp_diagnostic(&line_index, &diagnostic);

        assert_eq!(
            converted.range,
            Range::new(Position::new(0, 0), Position::new(0, 3))
        );
        assert_eq!(converted.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            converted.code,
            Some(NumberOrString::String(
                "syntax.unknown-document-kind".to_owned()
            ))
        );
        assert_eq!(
            converted.message,
            "expected a top-level MCDPL document kind\nhelp: Start the file with mcdp."
        );
    }

    #[test]
    fn syntax_diagnostics_parse_mcdpl_source() {
        let diagnostics = syntax_diagnostics("bad.mcdp", "bad { }");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            Some(NumberOrString::String(
                "syntax.unknown-document-kind".to_owned()
            ))
        );
    }

    #[test]
    fn document_diagnostics_include_compiler_lowering_errors() {
        let diagnostics = document_diagnostics(
            "file:///tmp/duplicate.mcdp",
            "\
mcdp {
  provides value [Nat]
  provides value [Nat]
}
",
            None,
        );

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.source.as_deref() == Some("mcdp-compiler")
                && diagnostic.code
                    == Some(NumberOrString::String("compiler.duplicate-port".to_owned()))
        }));
    }

    #[test]
    fn document_diagnostics_include_cross_namespace_duplicate_names() {
        let diagnostics = document_diagnostics(
            "file:///tmp/duplicate-name.mcdp",
            "\
dp {
  provides name [Nat]
  requires name [J]

  sub name = instance `model

  implemented-by yaml resource(\"test\")
}
",
            None,
        );

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.source.as_deref() == Some("mcdp-compiler")
                && diagnostic.code
                    == Some(NumberOrString::String("compiler.duplicate-name".to_owned()))
        }));
    }

    #[test]
    fn document_diagnostics_reject_trailing_tokens_after_port_posets() {
        let diagnostics = document_diagnostics(
            "file:///tmp/trailing-port-poset.mcdp",
            "\
dp {
  provides name [N];
  requires name [J]hallo;
}
",
            None,
        );

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.source.as_deref() == Some("mcdp-syntax")
                && diagnostic.code
                    == Some(NumberOrString::String(
                        "syntax.trailing-port-poset-token".to_owned(),
                    ))
        }));
    }
}
