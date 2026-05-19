//! Semantic-token encoding for syntax and symbol-aware highlighting.

use mcdp_language::{TextRange, TokenKind, lex};
use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensServerCapabilities, WorkDoneProgressOptions,
};

use crate::line_index::LineIndex;
use crate::project_symbols::{DocumentSymbols, InstanceScopedReference, PortDirection};

const TOKEN_TYPE_KEYWORD: u32 = 0;
const TOKEN_TYPE_VARIABLE: u32 = 1;
const TOKEN_TYPE_NUMBER: u32 = 2;
const TOKEN_TYPE_STRING: u32 = 3;
const TOKEN_TYPE_COMMENT: u32 = 4;
const TOKEN_TYPE_OPERATOR: u32 = 5;
const TOKEN_TYPE_PROVIDED_VARIABLE: u32 = 6;
const TOKEN_TYPE_REQUIRED_VARIABLE: u32 = 7;
const TOKEN_TYPE_INSTANCE: u32 = 8;
const TOKEN_TYPE_MODEL_REFERENCE: u32 = 9;
const TOKEN_TYPE_RESOURCE_BINDING: u32 = 10;

pub(crate) fn server_capabilities() -> SemanticTokensServerCapabilities {
    SemanticTokensServerCapabilities::SemanticTokensOptions(options())
}

pub(crate) fn options() -> SemanticTokensOptions {
    SemanticTokensOptions {
        work_done_progress_options: WorkDoneProgressOptions::default(),
        legend: legend(),
        range: Some(false),
        full: Some(SemanticTokensFullOptions::Bool(true)),
    }
}

fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::NUMBER,
            SemanticTokenType::STRING,
            SemanticTokenType::COMMENT,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::new("mcdplProvidedVariable"),
            SemanticTokenType::new("mcdplRequiredVariable"),
            SemanticTokenType::new("mcdplInstance"),
            SemanticTokenType::new("mcdplModelReference"),
            SemanticTokenType::new("mcdplResourceBinding"),
        ],
        token_modifiers: Vec::<SemanticTokenModifier>::new(),
    }
}

pub(crate) fn semantic_tokens(source: &str, symbols: Option<&DocumentSymbols>) -> SemanticTokens {
    let line_index = LineIndex::new(source);
    let roles = symbols.map(DocumentSemanticRoles::new);
    let mut previous_line = 0;
    let mut previous_start = 0;
    let mut data = Vec::new();

    for token in lex(source) {
        let Some(token_type) = roles
            .as_ref()
            .and_then(|roles| roles.token_type(&token.text, token.kind, token.range))
            .or_else(|| semantic_token_type(token.kind))
        else {
            continue;
        };

        for segment in line_index.token_segments(token.range) {
            if segment.length == 0 {
                continue;
            }

            let delta_line = segment.line.saturating_sub(previous_line);
            let delta_start = if delta_line == 0 {
                segment.start.saturating_sub(previous_start)
            } else {
                segment.start
            };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: segment.length,
                token_type,
                token_modifiers_bitset: 0,
            });
            previous_line = segment.line;
            previous_start = segment.start;
        }
    }

    SemanticTokens {
        result_id: None,
        data,
    }
}

struct DocumentSemanticRoles<'a> {
    symbols: &'a DocumentSymbols,
    instance_scoped_references: Vec<InstanceScopedReference>,
}

impl<'a> DocumentSemanticRoles<'a> {
    fn new(symbols: &'a DocumentSymbols) -> Self {
        Self {
            symbols,
            instance_scoped_references: symbols.instance_scoped_references(),
        }
    }

    fn token_type(&self, text: &str, kind: TokenKind, range: TextRange) -> Option<u32> {
        if self.is_resource_binding(range) {
            return Some(TOKEN_TYPE_RESOURCE_BINDING);
        }
        if self.is_model_reference_name(range) {
            return Some(TOKEN_TYPE_MODEL_REFERENCE);
        }
        if self.is_instance_name(text, kind, range) {
            return Some(TOKEN_TYPE_INSTANCE);
        }
        if let Some(direction) = self.instance_scoped_variable_direction(range) {
            return Some(match direction {
                PortDirection::Provided => TOKEN_TYPE_PROVIDED_VARIABLE,
                PortDirection::Required => TOKEN_TYPE_REQUIRED_VARIABLE,
            });
        }
        if self.is_provided_variable(text, kind, range) {
            return Some(TOKEN_TYPE_PROVIDED_VARIABLE);
        }
        if self.is_required_variable(text, kind, range) {
            return Some(TOKEN_TYPE_REQUIRED_VARIABLE);
        }

        None
    }

    fn is_resource_binding(&self, range: TextRange) -> bool {
        self.symbols
            .resource_bindings
            .iter()
            .any(|resource| contains_range(resource.declaration_range, range))
    }

    fn is_model_reference_name(&self, range: TextRange) -> bool {
        self.symbols
            .model_references
            .iter()
            .any(|reference| reference.name_range == range)
    }

    fn is_instance_name(&self, text: &str, kind: TokenKind, range: TextRange) -> bool {
        is_name_token(kind)
            && self
                .symbols
                .instances
                .iter()
                .any(|instance| instance.name_range == range || instance.name == text)
    }

    fn instance_scoped_variable_direction(&self, range: TextRange) -> Option<PortDirection> {
        self.instance_scoped_references
            .iter()
            .find(|reference| reference.port_range == range)
            .map(|reference| reference.direction)
    }

    fn is_provided_variable(&self, text: &str, kind: TokenKind, range: TextRange) -> bool {
        is_name_token(kind)
            && self
                .symbols
                .provides
                .iter()
                .any(|port| port.name_range == range || port.name == text)
    }

    fn is_required_variable(&self, text: &str, kind: TokenKind, range: TextRange) -> bool {
        is_name_token(kind)
            && self
                .symbols
                .requires
                .iter()
                .any(|port| port.name_range == range || port.name == text)
    }
}

fn semantic_token_type(kind: TokenKind) -> Option<u32> {
    match kind {
        TokenKind::Keyword => Some(TOKEN_TYPE_KEYWORD),
        TokenKind::Ident => Some(TOKEN_TYPE_VARIABLE),
        TokenKind::Number => Some(TOKEN_TYPE_NUMBER),
        TokenKind::String => Some(TOKEN_TYPE_STRING),
        TokenKind::Comment => Some(TOKEN_TYPE_COMMENT),
        TokenKind::Operator => Some(TOKEN_TYPE_OPERATOR),
        TokenKind::Whitespace
        | TokenKind::Newline
        | TokenKind::Punctuation
        | TokenKind::Unknown => None,
    }
}

fn contains_range(container: TextRange, candidate: TextRange) -> bool {
    candidate.start >= container.start && candidate.end <= container.end
}

fn is_name_token(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Ident | TokenKind::Keyword)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_symbols::DocumentSymbols;
    use tower_lsp::lsp_types::Url;

    #[test]
    fn options_advertise_full_document_tokens() {
        let options = options();

        assert_eq!(options.range, Some(false));
        assert_eq!(options.full, Some(SemanticTokensFullOptions::Bool(true)));
        assert_eq!(options.legend.token_types.len(), 11);
        assert!(options.legend.token_modifiers.is_empty());
    }

    #[test]
    fn encode_core_token_kinds() {
        let source = "\
mcdp {
  provides speed [m/s²]
  implemented-by yaml resource(\"catalog.yaml\") # catalog
  provided speed ≤ 10
}
";
        let tokens = absolute_tokens(&semantic_tokens(source, None).data);

        assert!(tokens.contains(&absolute_token(source, "mcdp", TOKEN_TYPE_KEYWORD)));
        assert!(tokens.contains(&absolute_token(source, "provides", TOKEN_TYPE_KEYWORD)));
        assert!(tokens.contains(&absolute_token(source, "speed", TOKEN_TYPE_VARIABLE)));
        assert!(tokens.contains(&absolute_token(
            source,
            "\"catalog.yaml\"",
            TOKEN_TYPE_STRING
        )));
        assert!(tokens.contains(&absolute_token(source, "# catalog", TOKEN_TYPE_COMMENT)));
        assert!(tokens.contains(&absolute_token(source, "≤", TOKEN_TYPE_OPERATOR)));
        assert!(tokens.contains(&absolute_token(source, "10", TOKEN_TYPE_NUMBER)));
    }

    #[test]
    fn split_multiline_string_segments() {
        let source = "mcdp {\n  label = \"a\nb\"\n}\n";
        let tokens = absolute_tokens(&semantic_tokens(source, None).data);
        let string_tokens: Vec<_> = tokens
            .into_iter()
            .filter(|token| token.token_type == TOKEN_TYPE_STRING)
            .collect();

        assert_eq!(
            string_tokens,
            vec![
                absolute_token(source, "\"a", TOKEN_TYPE_STRING),
                absolute_token(source, "b\"", TOKEN_TYPE_STRING),
            ]
        );
    }

    #[test]
    fn semantic_symbol_roles_override_lexical_variable_tokens() {
        let source = "\
mcdp {
  provides number_t1 [car]
  requires total_cost [USD]
  sub dp_t1 = instance `fleet_type_1
  implemented-by yaml resource(\"fleet.yaml\")
  provided number_t1 <= number_t1 provided by dp_t1
  required total_cost >= cost required by dp_t1
}
";
        let uri = test_file_url("/tmp/fleet.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token(
            source,
            "number_t1",
            TOKEN_TYPE_PROVIDED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token(
            source,
            "total_cost",
            TOKEN_TYPE_REQUIRED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token(source, "dp_t1", TOKEN_TYPE_INSTANCE)));
        assert!(tokens.contains(&absolute_token(
            source,
            "fleet_type_1",
            TOKEN_TYPE_MODEL_REFERENCE
        )));
        assert!(tokens.contains(&absolute_token(
            source,
            "implemented",
            TOKEN_TYPE_RESOURCE_BINDING
        )));
        assert!(tokens.contains(&absolute_token(
            source,
            "\"fleet.yaml\"",
            TOKEN_TYPE_RESOURCE_BINDING
        )));
        assert!(
            tokens.contains(&absolute_token_at_offset(
                source,
                "number_t1",
                source
                    .rfind("number_t1")
                    .expect("test source has number_t1"),
                TOKEN_TYPE_PROVIDED_VARIABLE
            ))
        );
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "dp_t1",
            source.rfind("dp_t1").expect("test source has dp_t1"),
            TOKEN_TYPE_INSTANCE
        )));
    }

    #[test]
    fn instance_scoped_variables_get_directional_role_colors() {
        let source = "\
mcdp {
  sub rs = instance `routing_service
  sub fu = instance `fuel
  sub mo = instance `monitor

  required total_cost >= (
    fuel_cost required by fu +
    monitor_cost required by mo
  )

  monitor_buses required by rs <= monitor_buses provided by mo
}
";
        let uri = test_file_url("/tmp/routing.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);
        let fuel_cost = offset_of(source, "fuel_cost required by fu");
        let monitor_cost = offset_of(source, "monitor_cost required by mo");
        let required_monitor_buses = offset_of(source, "monitor_buses required by rs");
        let provided_monitor_buses = offset_of(source, "monitor_buses provided by mo");

        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "fuel_cost",
            fuel_cost,
            TOKEN_TYPE_REQUIRED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "monitor_cost",
            monitor_cost,
            TOKEN_TYPE_REQUIRED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "monitor_buses",
            required_monitor_buses,
            TOKEN_TYPE_REQUIRED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "monitor_buses",
            provided_monitor_buses,
            TOKEN_TYPE_PROVIDED_VARIABLE
        )));
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct AbsoluteToken {
        line: u32,
        start: u32,
        length: u32,
        token_type: u32,
    }

    fn absolute_tokens(tokens: &[SemanticToken]) -> Vec<AbsoluteToken> {
        let mut line = 0;
        let mut start = 0;
        tokens
            .iter()
            .map(|token| {
                line += token.delta_line;
                if token.delta_line == 0 {
                    start += token.delta_start;
                } else {
                    start = token.delta_start;
                }
                AbsoluteToken {
                    line,
                    start,
                    length: token.length,
                    token_type: token.token_type,
                }
            })
            .collect()
    }

    fn absolute_token(source: &str, needle: &str, token_type: u32) -> AbsoluteToken {
        absolute_token_at_offset(source, needle, offset_of(source, needle), token_type)
    }

    fn absolute_token_at_offset(
        source: &str,
        needle: &str,
        start_offset: usize,
        token_type: u32,
    ) -> AbsoluteToken {
        let line_index = LineIndex::new(source);
        let start = line_index.position(start_offset);
        let end = line_index.position(start_offset + needle.len());

        AbsoluteToken {
            line: start.line,
            start: start.character,
            length: end.character.saturating_sub(start.character),
            token_type,
        }
    }

    fn test_file_url(path: &str) -> Url {
        match Url::from_file_path(path) {
            Ok(url) => url,
            Err(()) => panic!("could not convert test path to file URL"),
        }
    }

    fn offset_of(source: &str, needle: &str) -> usize {
        match source.find(needle) {
            Some(offset) => offset,
            None => panic!("missing `{needle}` in test source"),
        }
    }
}
