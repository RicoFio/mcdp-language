//! Semantic-token encoding for syntax and symbol-aware highlighting.

use mcdp_language::{DocumentKind, TextRange, TokenKind, lex};
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
const TOKEN_TYPE_DOCUMENT_KIND: u32 = 11;
const TOKEN_TYPE_TEMPLATE_REFERENCE: u32 = 12;
const TOKEN_TYPE_LIBRARY: u32 = 13;

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
            SemanticTokenType::new("mcdplDocumentKind"),
            SemanticTokenType::new("mcdplTemplateReference"),
            SemanticTokenType::new("mcdplLibrary"),
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
        let role_decision = roles
            .as_ref()
            .and_then(|roles| roles.token_type(&token.text, token.kind, token.range));
        let token_type = match role_decision {
            Some(RoleDecision::Token(token_type)) => Some(token_type),
            Some(RoleDecision::Suppress) => None,
            None => semantic_token_type(token.kind),
        };
        let Some(token_type) = token_type else {
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

enum RoleDecision {
    Token(u32),
    Suppress,
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

    fn token_type(&self, text: &str, kind: TokenKind, range: TextRange) -> Option<RoleDecision> {
        if self.is_resource_path(kind, range) {
            return Some(RoleDecision::Suppress);
        }
        if self.is_document_kind(text, kind, range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_DOCUMENT_KIND));
        }
        if self.is_resource_binding_prefix(kind, range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_RESOURCE_BINDING));
        }
        if self.is_specialize_template_target(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_TEMPLATE_REFERENCE));
        }
        if self.is_instance_specialize_template_target(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_TEMPLATE_REFERENCE));
        }
        if self.is_import_library_name(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_LIBRARY));
        }
        if self.is_import_name(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_MODEL_REFERENCE));
        }
        if self.is_instance_model_reference_name(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_INSTANCE));
        }
        if self.is_specialize_parameter_reference(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_INSTANCE));
        }
        if self.is_model_reference_name(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_MODEL_REFERENCE));
        }
        if let Some(direction) = self.instance_scoped_variable_direction(range) {
            return Some(RoleDecision::Token(match direction {
                PortDirection::Provided => TOKEN_TYPE_PROVIDED_VARIABLE,
                PortDirection::Required => TOKEN_TYPE_REQUIRED_VARIABLE,
            }));
        }
        if self.is_provided_variable_declaration(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_PROVIDED_VARIABLE));
        }
        if self.is_required_variable_declaration(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_REQUIRED_VARIABLE));
        }
        if self.is_instance_name_declaration(range) {
            return Some(RoleDecision::Token(TOKEN_TYPE_INSTANCE));
        }
        if self.is_instance_name_reference(text, kind) {
            return Some(RoleDecision::Token(TOKEN_TYPE_INSTANCE));
        }
        if self.is_provided_variable_reference(text, kind) {
            return Some(RoleDecision::Token(TOKEN_TYPE_PROVIDED_VARIABLE));
        }
        if self.is_required_variable_reference(text, kind) {
            return Some(RoleDecision::Token(TOKEN_TYPE_REQUIRED_VARIABLE));
        }

        None
    }

    fn is_resource_path(&self, kind: TokenKind, range: TextRange) -> bool {
        kind == TokenKind::String
            && self
                .symbols
                .resource_bindings
                .iter()
                .any(|resource| contains_range(range, resource.path_range))
    }

    fn is_document_kind(&self, text: &str, kind: TokenKind, range: TextRange) -> bool {
        matches!(kind, TokenKind::Ident | TokenKind::Keyword)
            && matches!(text, "dp" | "mcdp")
            && self.symbols.definition_range == range
    }

    fn is_resource_binding_prefix(&self, kind: TokenKind, range: TextRange) -> bool {
        if matches!(
            kind,
            TokenKind::Whitespace
                | TokenKind::Newline
                | TokenKind::String
                | TokenKind::Comment
                | TokenKind::Number
                | TokenKind::Unknown
        ) {
            return false;
        }

        self.symbols.resource_bindings.iter().any(|resource| {
            let prefix_range =
                TextRange::new(resource.declaration_range.start, resource.path_range.start);
            contains_range(prefix_range, range)
        })
    }

    fn is_instance_model_reference_name(&self, range: TextRange) -> bool {
        self.symbols.instances.iter().any(|instance| {
            instance
                .model
                .as_ref()
                .is_some_and(|model| model.name_range == range)
                || self.symbols.model_references.iter().any(|reference| {
                    reference.name_range == range
                        && contains_range(instance.declaration_range, reference.reference_range)
                })
        })
    }

    fn is_model_reference_name(&self, range: TextRange) -> bool {
        self.symbols
            .model_references
            .iter()
            .any(|reference| reference.name_range == range)
    }

    /// The template a `specialize [...] \`Template` statement instantiates,
    /// highlighted distinctly from the concrete instances bound to it.
    fn is_specialize_template_target(&self, range: TextRange) -> bool {
        self.symbols
            .specialize_target
            .as_ref()
            .is_some_and(|target| target.name_range == range)
    }

    /// The template targeted by an embedded `instance specialize [...] \`Template`
    /// binding, highlighted the same as top-level specialize documents rather
    /// than as a plain instance.
    fn is_instance_specialize_template_target(&self, range: TextRange) -> bool {
        self.symbols.instances.iter().any(|instance| {
            instance.specializes_template
                && instance
                    .model
                    .as_ref()
                    .is_some_and(|model| model.name_range == range)
        })
    }

    /// The library name in a `from library <lib> import interface <Name>` statement.
    fn is_import_library_name(&self, range: TextRange) -> bool {
        self.symbols
            .imports
            .iter()
            .any(|import| import.library_range == Some(range))
    }

    /// The imported symbol name(s) in a `from library <lib> import interface <Name>` statement.
    fn is_import_name(&self, range: TextRange) -> bool {
        self.symbols
            .imports
            .iter()
            .any(|import| import.imported_name_ranges.contains(&range))
    }

    /// Concrete model/interface bindings inside a `specialize [Name: \`model, ...]`
    /// parameter list, highlighted the same as other instance references.
    fn is_specialize_parameter_reference(&self, range: TextRange) -> bool {
        self.symbols.kind == Some(DocumentKind::Specialize)
            && self.is_model_reference_name(range)
            && !self.is_specialize_template_target(range)
    }

    fn is_instance_name_declaration(&self, range: TextRange) -> bool {
        self.symbols
            .instances
            .iter()
            .any(|instance| instance.name_range == range)
    }

    fn is_instance_name_reference(&self, text: &str, kind: TokenKind) -> bool {
        is_name_token(kind)
            && self
                .symbols
                .instances
                .iter()
                .any(|instance| instance.name == text)
    }

    fn instance_scoped_variable_direction(&self, range: TextRange) -> Option<PortDirection> {
        self.instance_scoped_references
            .iter()
            .find(|reference| reference.port_range == range)
            .map(|reference| reference.direction)
    }

    fn is_provided_variable_declaration(&self, range: TextRange) -> bool {
        self.symbols
            .provides
            .iter()
            .any(|port| port.name_range == range)
    }

    fn is_provided_variable_reference(&self, text: &str, kind: TokenKind) -> bool {
        is_name_token(kind) && self.symbols.provides.iter().any(|port| port.name == text)
    }

    fn is_required_variable_declaration(&self, range: TextRange) -> bool {
        self.symbols
            .requires
            .iter()
            .any(|port| port.name_range == range)
    }

    fn is_required_variable_reference(&self, text: &str, kind: TokenKind) -> bool {
        is_name_token(kind) && self.symbols.requires.iter().any(|port| port.name == text)
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
        assert_eq!(options.legend.token_types.len(), 14);
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
  requires student_policy [`student_policy]
  sub dp_t1 = instance `fleet_type_1
  implemented-by yaml resource(\"fleet.yaml\")
  provided number_t1 <= number_t1 provided by dp_t1
  required total_cost >= cost required by dp_t1
}
";
        let uri = test_file_url("/tmp/fleet.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token(source, "mcdp", TOKEN_TYPE_DOCUMENT_KIND)));
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
        assert!(tokens.contains(&absolute_token(source, "fleet_type_1", TOKEN_TYPE_INSTANCE)));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "student_policy",
            offset_of(source, "`student_policy") + 1,
            TOKEN_TYPE_MODEL_REFERENCE
        )));
        assert!(tokens.contains(&absolute_token(
            source,
            "implemented",
            TOKEN_TYPE_RESOURCE_BINDING
        )));
        assert!(!tokens.contains(&absolute_token(
            source,
            "\"fleet.yaml\"",
            TOKEN_TYPE_RESOURCE_BINDING
        )));
        assert!(!tokens.contains(&absolute_token(source, "\"fleet.yaml\"", TOKEN_TYPE_STRING)));
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
    fn duplicate_name_declarations_keep_their_declaration_role_colors() {
        let source = "\
dp {
  provides name [Nat]
  requires name [J]

  sub name = instance `model

  implemented-by yaml resource(\"test\")
}
";
        let uri = test_file_url("/tmp/duplicate-name.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "name",
            offset_after(source, "provides "),
            TOKEN_TYPE_PROVIDED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "name",
            offset_after(source, "requires "),
            TOKEN_TYPE_REQUIRED_VARIABLE
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "name",
            offset_after(source, "sub "),
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

    #[test]
    fn instance_model_references_inside_instance_declarations_are_instance_colored() {
        let source = "\
mcdp {
  provides f [Nat]
  requires r [Nat]

  sub n0 = instance `unit
  sub n1 = instance `unit
  sub n2 = instance `unit
}
";
        let uri = test_file_url("/tmp/units.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);
        let mut search_start = 0;

        for _ in 0..3 {
            let unit_offset = search_start
                + source[search_start..]
                    .find("`unit")
                    .expect("test source has instance unit reference")
                + 1;
            assert!(tokens.contains(&absolute_token_at_offset(
                source,
                "unit",
                unit_offset,
                TOKEN_TYPE_INSTANCE
            )));
            search_start = unit_offset + "unit".len();
        }
    }

    #[test]
    fn specialize_template_target_and_parameter_bindings_get_distinct_colors() {
        let source = "\
specialize [
  Battery: `batteries_uncertain1.batteries,
  Actuation: `actuations_v2.actuation
] `UAVCompleteTemplate
";
        let uri = test_file_url("/tmp/uav_actuation_battery.mcdp");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token(
            source,
            "UAVCompleteTemplate",
            TOKEN_TYPE_TEMPLATE_REFERENCE
        )));
        let batteries_offset = offset_after(source, "batteries_uncertain1.");
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "batteries",
            batteries_offset,
            TOKEN_TYPE_INSTANCE
        )));
        assert!(!tokens.contains(&absolute_token_at_offset(
            source,
            "batteries",
            batteries_offset,
            TOKEN_TYPE_MODEL_REFERENCE
        )));
    }

    #[test]
    fn instance_specialize_template_target_is_template_colored_not_instance() {
        let source = "\
template [Battery: BatteryInterface]
mcdp {
  actuation_energetics = instance specialize [
    Battery: Battery
  ] `ActuationEnergeticsTemplate
}
";
        let uri = test_file_url("/tmp/UAVCompleteTemplate.mcdp_template");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token(
            source,
            "ActuationEnergeticsTemplate",
            TOKEN_TYPE_TEMPLATE_REFERENCE
        )));
        assert!(!tokens.contains(&absolute_token(
            source,
            "ActuationEnergeticsTemplate",
            TOKEN_TYPE_INSTANCE
        )));
    }

    #[test]
    fn from_library_import_statements_color_library_and_imported_names() {
        let source = "\
from library batteries_uncertain1 import interface BatteryInterface

template [Battery: BatteryInterface]
mcdp {
  provides range [km]
}
";
        let uri = test_file_url("/tmp/UAVCompleteTemplate.mcdp_template");
        let symbols = DocumentSymbols::parse(uri, source);
        let tokens = absolute_tokens(&semantic_tokens(source, Some(&symbols)).data);

        assert!(tokens.contains(&absolute_token(
            source,
            "batteries_uncertain1",
            TOKEN_TYPE_LIBRARY
        )));
        assert!(tokens.contains(&absolute_token_at_offset(
            source,
            "BatteryInterface",
            offset_after(source, "import interface "),
            TOKEN_TYPE_MODEL_REFERENCE
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

    fn offset_after(source: &str, needle: &str) -> usize {
        offset_of(source, needle) + needle.len()
    }
}
