//! Statement-level lowering.
//!
//! This module consumes the recovered syntax tree and builds a typed but still
//! source-preserving semantic model. It deliberately keeps expressions as text
//! until the expression/unit parsers are ready.

use std::collections::BTreeMap;

use crate::{
    Constraint, DesignGraph, Diagnostic, DocumentKind, Expression, NamedPoset, Node,
    ParsedDocument, Port, PortDirection, PosetRef, Relation, SourceId, Statement, StatementKind,
    SyntaxBody, SyntaxEntry, TextRange, TextSpan, parse_expression_list_text,
    parse_expression_text, parse_unit_expression_text,
};

/// Semantic representation recovered from one source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticModel {
    /// Source file or virtual document.
    pub source: SourceId,
    /// Top-level document kind.
    pub kind: DocumentKind,
    /// Public interface declarations.
    pub ports: Vec<PortDecl>,
    /// Subproblem instances.
    pub instances: Vec<InstanceDecl>,
    /// Formula aliases and constants.
    pub assignments: Vec<AssignmentDecl>,
    /// Order/refinement constraints.
    pub constraints: Vec<ConstraintDecl>,
    /// External implementation bindings.
    pub implementations: Vec<ImplementationDecl>,
    /// Implemented interface declarations.
    pub interfaces: Vec<InterfaceImplDecl>,
    /// Inline catalog records.
    pub catalog_records: Vec<CatalogRecordDecl>,
    /// Labeled entries from `choose (...)` or `intersection (...)` documents.
    pub choices: Vec<ChoiceDecl>,
    /// Explicit import declarations.
    pub imports: Vec<ImportDecl>,
    /// Template or specialization parameters preserved as source text.
    pub parameters: Vec<ParameterDecl>,
    /// Statements not yet understood by lowering.
    pub bare_expressions: Vec<BareExpressionDecl>,
}

impl SemanticModel {
    fn new(source: SourceId, kind: DocumentKind) -> Self {
        Self {
            source,
            kind,
            ports: Vec::new(),
            instances: Vec::new(),
            assignments: Vec::new(),
            constraints: Vec::new(),
            implementations: Vec::new(),
            interfaces: Vec::new(),
            catalog_records: Vec::new(),
            choices: Vec::new(),
            imports: Vec::new(),
            parameters: Vec::new(),
            bare_expressions: Vec::new(),
        }
    }
}

/// Public interface declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortDecl {
    /// Port name.
    pub name: String,
    /// Port direction.
    pub direction: PortDirection,
    /// Declared poset/type expression.
    pub poset: PosetRef,
    /// Source span.
    pub span: TextSpan,
}

/// Subproblem instance declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceDecl {
    /// Local instance name.
    pub name: String,
    /// Referenced model name.
    pub model: String,
    /// Source span.
    pub span: TextSpan,
}

/// Formula assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentDecl {
    /// Assigned name or expression.
    pub target: String,
    /// Right-hand-side expression.
    pub expression: String,
    /// Parsed right-hand-side expression.
    pub expression_ast: Expression,
    /// Source span.
    pub span: TextSpan,
}

/// Constraint declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstraintDecl {
    /// Left expression text.
    pub left: String,
    /// Parsed left expression.
    pub left_expr: Expression,
    /// Relation.
    pub relation: Relation,
    /// Right expression text.
    pub right: String,
    /// Parsed right expression.
    pub right_expr: Expression,
    /// Source span.
    pub span: TextSpan,
}

/// External implementation binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationDecl {
    /// Backend identifier, for example `yaml`.
    pub backend: String,
    /// Optional resource path.
    pub resource: Option<String>,
    /// Raw implementation text after `implemented-by`.
    pub raw: String,
    /// Source span.
    pub span: TextSpan,
}

/// Interface implementation declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceImplDecl {
    /// Referenced interface model name.
    pub name: String,
    /// Source span.
    pub span: TextSpan,
}

/// Inline catalog row split around its implementation marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogRecordDecl {
    /// Functionality values provided by this implementation.
    pub provides: String,
    /// Parsed functionality values.
    pub provided_values: Vec<Expression>,
    /// Implementation identifier.
    pub implementation: String,
    /// Requirement values needed by this implementation.
    pub requires: String,
    /// Parsed requirement values.
    pub required_values: Vec<Expression>,
    /// Source span.
    pub span: TextSpan,
}

/// Labeled target declaration in a parenthesized composition document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChoiceDecl {
    /// Entry label.
    pub name: String,
    /// Referenced model.
    pub target: String,
    /// Source span.
    pub span: TextSpan,
}

/// Import declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportDecl {
    /// Raw import statement.
    pub raw: String,
    /// Imported model names recovered from the statement.
    pub models: Vec<String>,
    /// Source span.
    pub span: TextSpan,
}

/// Template or specialization parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterDecl {
    /// Raw parameter text.
    pub text: String,
    /// Source span.
    pub span: TextSpan,
}

/// Preserved source statement that lowering does not interpret yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BareExpressionDecl {
    /// Raw expression text.
    pub text: String,
    /// Source span.
    pub span: TextSpan,
}

/// Lowers a parsed document into a source-preserving semantic model.
#[must_use]
pub fn lower_document(
    source: SourceId,
    parsed: &ParsedDocument,
) -> (Option<SemanticModel>, Vec<Diagnostic>) {
    let Some(kind) = parsed.kind else {
        return (
            None,
            vec![Diagnostic::error(
                "compiler.missing-document-kind",
                "cannot lower a document without a recognized top-level kind",
            )],
        );
    };

    let Some(syntax) = &parsed.syntax else {
        return (
            None,
            vec![Diagnostic::error(
                "compiler.missing-syntax",
                "cannot lower a document without a recovered syntax tree",
            )],
        );
    };

    let mut model = SemanticModel::new(source.clone(), kind);
    let mut diagnostics = Vec::new();

    match &syntax.body {
        SyntaxBody::Braced { statements } => {
            lower_statements(&source, statements, &mut model, &mut diagnostics);
        }
        SyntaxBody::Parenthesized { entries } => {
            lower_choices(&source, entries, &mut model, &mut diagnostics);
        }
        SyntaxBody::Template {
            parameters,
            statements,
        } => {
            lower_parameters(&source, parameters, &mut model);
            lower_statements(&source, statements, &mut model, &mut diagnostics);
        }
        SyntaxBody::Specialize { parameters, target } => {
            lower_parameters(&source, parameters, &mut model);
            if let Some(target) = target {
                model.bare_expressions.push(BareExpressionDecl {
                    text: target.text.clone(),
                    span: span(source.clone(), target.range),
                });
            }
        }
        SyntaxBody::Empty => diagnostics.push(Diagnostic::error(
            "compiler.empty-syntax-body",
            "document body could not be recovered for lowering",
        )),
    }

    detect_duplicate_declarations(&model, &mut diagnostics);
    (Some(model), diagnostics)
}

/// Builds the current graph shell from the lowered semantic model.
#[must_use]
pub fn graph_from_semantic(name: Option<String>, model: &SemanticModel) -> DesignGraph {
    let ports = model
        .ports
        .iter()
        .map(|port| Port::new(port.name.clone(), port.direction, port.poset.clone()))
        .collect();
    let nodes = model
        .instances
        .iter()
        .map(|instance| Node::new(instance.name.clone(), instance.model.clone()))
        .collect();
    let mut constraints: Vec<Constraint> = model
        .constraints
        .iter()
        .map(|constraint| {
            Constraint::new(
                constraint.left.clone(),
                constraint.relation,
                constraint.right.clone(),
            )
        })
        .collect();
    constraints.extend(model.assignments.iter().map(|assignment| {
        Constraint::new(
            assignment.target.clone(),
            Relation::Eq,
            assignment.expression.clone(),
        )
    }));

    DesignGraph {
        name,
        ports,
        nodes,
        constraints,
    }
}

fn lower_statements(
    source: &SourceId,
    statements: &[Statement],
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement.kind {
            StatementKind::Provides => {
                lower_port(
                    source,
                    statement,
                    PortDirection::Provides,
                    model,
                    diagnostics,
                );
            }
            StatementKind::Requires => {
                lower_port(
                    source,
                    statement,
                    PortDirection::Requires,
                    model,
                    diagnostics,
                );
            }
            StatementKind::Instance => lower_instance(source, statement, model, diagnostics),
            StatementKind::Assignment => lower_assignment(source, statement, model, diagnostics),
            StatementKind::Constraint => lower_constraint(source, statement, model, diagnostics),
            StatementKind::ImplementedBy => {
                lower_implementation(source, statement, model, diagnostics);
            }
            StatementKind::Implements => {
                lower_interface_impl(source, statement, model, diagnostics)
            }
            StatementKind::CatalogRecord => {
                lower_catalog_record(source, statement, model, diagnostics);
            }
            StatementKind::Import => lower_import(source, statement, model, diagnostics),
            StatementKind::BareExpression => {
                model.bare_expressions.push(BareExpressionDecl {
                    text: statement.text.clone(),
                    span: span(source.clone(), statement.range),
                });
            }
        }
    }
}

fn lower_port(
    source: &SourceId,
    statement: &Statement,
    direction: PortDirection,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_port_decl(&statement.text, direction) {
        Some((name, poset)) => model.ports.push(PortDecl {
            name,
            direction,
            poset,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-port",
                "expected a port declaration like `provides name [poset]` or `requires name [poset]`",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_instance(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_instance_decl(&statement.text) {
        Some((name, referenced_model)) => model.instances.push(InstanceDecl {
            name,
            model: referenced_model,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-instance",
                "expected an instance declaration like `local = instance `model`",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_assignment(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match split_assignment(&statement.text) {
        Some((target, expression)) => model.assignments.push(AssignmentDecl {
            target,
            expression_ast: parse_expression_text(&expression),
            expression,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-assignment",
                "expected an assignment with non-empty left and right sides",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_constraint(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match constraint_decl_from_text(source, statement, &statement.text) {
        Some(constraint) => model.constraints.push(constraint),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-constraint",
                "expected a constraint with a supported relation operator",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn constraint_decl_from_text(
    source: &SourceId,
    statement: &Statement,
    text: &str,
) -> Option<ConstraintDecl> {
    split_relation(text).map(|(left, relation, right)| ConstraintDecl {
        left_expr: parse_expression_text(&left),
        left,
        relation,
        right_expr: parse_expression_text(&right),
        right,
        span: span(source.clone(), statement.range),
    })
}

fn lower_implementation(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_implementation_decl(&statement.text) {
        Some(implementation) => model.implementations.push(ImplementationDecl {
            backend: implementation.backend,
            resource: implementation.resource,
            raw: implementation.raw,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-implementation",
                "expected `implemented-by <backend> resource(\"path\")`",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_interface_impl(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_interface_impl_decl(&statement.text) {
        Some(name) => model.interfaces.push(InterfaceImplDecl {
            name,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-interface-implementation",
                "expected `implements `interface_name`",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_catalog_record(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_catalog_record(&statement.text) {
        Some((provides, implementation, requires)) => {
            model.catalog_records.push(CatalogRecordDecl {
                provided_values: parse_expression_list_text(&provides),
                provides,
                implementation,
                required_values: parse_expression_list_text(&requires),
                requires,
                span: span(source.clone(), statement.range),
            });
        }
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-catalog-record",
                "expected an inline catalog row with implementation arrows",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_import(
    source: &SourceId,
    statement: &Statement,
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match parse_import_decl(&statement.text) {
        Some(models) => model.imports.push(ImportDecl {
            raw: statement.text.clone(),
            models,
            span: span(source.clone(), statement.range),
        }),
        None => diagnostics.push(
            Diagnostic::error(
                "compiler.malformed-import",
                "expected an import declaration containing `import model ...`",
            )
            .with_span(span(source.clone(), statement.range)),
        ),
    }
}

fn lower_choices(
    source: &SourceId,
    entries: &[SyntaxEntry],
    model: &mut SemanticModel,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for entry in entries {
        match parse_choice_decl(&entry.text) {
            Some((name, target)) => model.choices.push(ChoiceDecl {
                name,
                target,
                span: span(source.clone(), entry.range),
            }),
            None => diagnostics.push(
                Diagnostic::error(
                    "compiler.malformed-choice",
                    "expected a choice entry like `Name: `Model`",
                )
                .with_span(span(source.clone(), entry.range)),
            ),
        }
    }
}

fn lower_parameters(source: &SourceId, entries: &[SyntaxEntry], model: &mut SemanticModel) {
    model
        .parameters
        .extend(entries.iter().map(|entry| ParameterDecl {
            text: entry.text.clone(),
            span: span(source.clone(), entry.range),
        }));
}

fn detect_duplicate_declarations(model: &SemanticModel, diagnostics: &mut Vec<Diagnostic>) {
    let mut ports = BTreeMap::<String, TextSpan>::new();
    for port in &model.ports {
        let direction = match port.direction {
            PortDirection::Provides => "provides",
            PortDirection::Requires => "requires",
        };
        let key = format!("{direction}:{}", port.name);
        if let Some(first_span) = ports.insert(key, port.span.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "compiler.duplicate-port",
                    format!("duplicate {direction} port `{}`", port.name),
                )
                .with_span(first_span)
                .with_help("Use unique port names within each direction."),
            );
        }
    }

    let mut names = BTreeMap::<String, (&'static str, TextSpan)>::new();
    for port in &model.ports {
        let role = match port.direction {
            PortDirection::Provides => "provided port",
            PortDirection::Requires => "required port",
        };
        detect_duplicate_name(&mut names, diagnostics, &port.name, role, &port.span);
    }

    let mut instances = BTreeMap::<String, TextSpan>::new();
    for instance in &model.instances {
        if let Some(first_span) = instances.insert(instance.name.clone(), instance.span.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "compiler.duplicate-instance",
                    format!("duplicate instance `{}`", instance.name),
                )
                .with_span(first_span)
                .with_help("Use unique local instance names."),
            );
        }
        detect_duplicate_name(
            &mut names,
            diagnostics,
            &instance.name,
            "instance",
            &instance.span,
        );
    }
}

fn detect_duplicate_name(
    names: &mut BTreeMap<String, (&'static str, TextSpan)>,
    diagnostics: &mut Vec<Diagnostic>,
    name: &str,
    role: &'static str,
    span: &TextSpan,
) {
    match names.get(name) {
        Some((first_role, _)) if *first_role != role => diagnostics.push(
            Diagnostic::error(
                "compiler.duplicate-name",
                format!("name `{name}` is already used as a {first_role}"),
            )
            .with_span(span.clone())
            .with_help(
                "Use distinct names for provided ports, required ports, and local instances.",
            ),
        ),
        Some(_) => {}
        None => {
            names.insert(name.to_owned(), (role, span.clone()));
        }
    }
}

fn parse_port_decl(text: &str, direction: PortDirection) -> Option<(String, PosetRef)> {
    let keyword = match direction {
        PortDirection::Provides => "provides",
        PortDirection::Requires => "requires",
    };
    let rest = text.trim().strip_prefix(keyword)?.trim();
    let bracket_start = rest.find('[')?;
    let name = rest[..bracket_start].trim();
    if name.is_empty() || name.split_whitespace().count() != 1 {
        return None;
    }
    let poset = extract_bracketed(&rest[bracket_start..]).map(parse_poset_ref)?;
    Some((name.to_owned(), poset))
}

fn parse_poset_ref(text: &str) -> PosetRef {
    let normalized = normalize_inline_whitespace(text);
    if let Some(named) = normalized.strip_prefix('`') {
        return PosetRef::Named(named.trim().to_owned());
    }

    if let Some(inner) = normalized
        .strip_prefix("product(")
        .and_then(|rest| rest.strip_suffix(')'))
        && let Some(fields) = parse_product_fields(inner)
    {
        return PosetRef::Product(fields);
    }

    PosetRef::Unit(parse_unit_expression_text(&normalized))
}

fn parse_product_fields(text: &str) -> Option<Vec<NamedPoset>> {
    let mut fields = Vec::new();
    for field in split_top_level(text, ',') {
        let (name, poset_text) = split_top_level_once(&field, ':')?;
        let field_name = name.trim();
        let field_poset = poset_text.trim();
        if field_name.is_empty() || field_poset.is_empty() {
            return None;
        }
        fields.push(NamedPoset::new(field_name, parse_poset_ref(field_poset)));
    }
    Some(fields)
}

fn parse_instance_decl(text: &str) -> Option<(String, String)> {
    let instance_index = text.find("instance")?;
    let before = text[..instance_index].trim();
    let before = before.strip_prefix("sub").unwrap_or(before).trim();
    let name = before.split('=').next()?.trim();
    let target = parse_model_ref(&text[instance_index + "instance".len()..])?;
    if name.is_empty() || target.is_empty() {
        return None;
    }
    Some((name.to_owned(), target))
}

fn parse_model_ref(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches('`').trim();
    let model = trimmed
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ')' | ']'))
        .next()?
        .trim_start_matches('`')
        .trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_owned())
    }
}

fn split_assignment(text: &str) -> Option<(String, String)> {
    let index = find_top_level_operator(text, &["="])?;
    let left = normalize_assignment_target(text[..index].trim());
    let right = text[index + 1..].trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right.to_owned()))
}

fn normalize_assignment_target(text: &str) -> String {
    text.strip_prefix("constant")
        .map(str::trim)
        .unwrap_or(text)
        .to_owned()
}

fn split_relation(text: &str) -> Option<(String, Relation, String)> {
    let relations = ["<=", ">=", "≤", "≥", "⪯", "⪰"];
    let index = find_top_level_operator(text, &relations)?;
    let operator = relations
        .iter()
        .find(|operator| text[index..].starts_with(**operator))?;
    let left = text[..index].trim();
    let right = text[index + operator.len()..].trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let relation = match *operator {
        "<=" | "≤" | "⪯" => Relation::Leq,
        ">=" | "≥" | "⪰" => Relation::Geq,
        _ => return None,
    };
    Some((left.to_owned(), relation, right.to_owned()))
}

fn find_top_level_operator(text: &str, operators: &[&str]) -> Option<usize> {
    let mut depth = DelimiterDepth::default();
    for (index, ch) in text.char_indices() {
        depth.observe(ch);
        if depth.is_zero()
            && operators
                .iter()
                .any(|operator| text[index..].starts_with(operator))
        {
            return Some(index);
        }
    }
    None
}

struct ParsedImplementation {
    backend: String,
    resource: Option<String>,
    raw: String,
}

fn parse_implementation_decl(text: &str) -> Option<ParsedImplementation> {
    let rest = text.trim().strip_prefix("implemented-by")?.trim();
    let backend = rest.split_whitespace().next()?.to_owned();
    Some(ParsedImplementation {
        backend,
        resource: extract_resource_path(rest),
        raw: rest.to_owned(),
    })
}

fn parse_interface_impl_decl(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("implements")?.trim();
    parse_model_ref(rest)
}

fn extract_resource_path(text: &str) -> Option<String> {
    let start = text.find("resource(")? + "resource(".len();
    let inside = text[start..].trim();
    let quote = inside.chars().find(|ch| *ch == '"' || *ch == '\'')?;
    let after_quote = inside.get(quote.len_utf8()..)?;
    let end = after_quote.find(quote)?;
    Some(after_quote[..end].to_owned())
}

fn parse_catalog_record(text: &str) -> Option<(String, String, String)> {
    let left_arrow = find_first_marker(text, &["<--|", "<-|", "↤"])?;
    let right_search_start = left_arrow.index + left_arrow.marker.len();
    let right_arrow = find_first_marker(&text[right_search_start..], &["|-->", "|->", "↦"])?;
    let right_index = right_search_start + right_arrow.index;
    let provides = text[..left_arrow.index].trim();
    let implementation = text[right_search_start..right_index].trim();
    let requires = text[right_index + right_arrow.marker.len()..].trim();
    if provides.is_empty() || implementation.is_empty() || requires.is_empty() {
        return None;
    }
    Some((
        provides.to_owned(),
        implementation.to_owned(),
        requires.to_owned(),
    ))
}

fn parse_choice_decl(text: &str) -> Option<(String, String)> {
    let (name, target) = split_top_level_once(text, ':')?;
    let name = name.trim();
    let target = parse_model_ref(target.trim())?;
    if name.is_empty() {
        return None;
    }
    Some((name.to_owned(), target))
}

fn parse_import_decl(text: &str) -> Option<Vec<String>> {
    let import_index = text.find("import")?;
    let imported = text[import_index + "import".len()..].replace(',', " ");
    let models: Vec<String> = imported
        .split_whitespace()
        .filter(|part| !matches!(*part, "model" | "models"))
        .map(|part| part.trim_start_matches('`').to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

fn extract_bracketed(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(text[start + 1..end].trim())
}

fn normalize_inline_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn split_top_level(text: &str, delimiter: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = DelimiterDepth::default();
    let mut start = 0usize;

    for (index, ch) in text.char_indices() {
        depth.observe(ch);
        if depth.is_zero() && ch == delimiter {
            parts.push(text[start..index].trim().to_owned());
            start = index + ch.len_utf8();
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_owned());
    }
    parts
}

fn split_top_level_once(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let mut depth = DelimiterDepth::default();
    for (index, ch) in text.char_indices() {
        depth.observe(ch);
        if depth.is_zero() && ch == delimiter {
            return Some((&text[..index], &text[index + ch.len_utf8()..]));
        }
    }
    None
}

#[derive(Clone, Copy)]
struct MarkerMatch<'a> {
    index: usize,
    marker: &'a str,
}

fn find_first_marker<'a>(text: &str, markers: &'a [&str]) -> Option<MarkerMatch<'a>> {
    markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|index| MarkerMatch { index, marker }))
        .min_by_key(|found| found.index)
}

fn span(source: SourceId, range: TextRange) -> TextSpan {
    TextSpan::new(source, range)
}

#[derive(Default)]
struct DelimiterDepth {
    braces: usize,
    parentheses: usize,
    brackets: usize,
    angles: usize,
}

impl DelimiterDepth {
    fn observe(&mut self, ch: char) {
        match ch {
            '{' => self.braces += 1,
            '}' => self.braces = self.braces.saturating_sub(1),
            '(' => self.parentheses += 1,
            ')' => self.parentheses = self.parentheses.saturating_sub(1),
            '[' => self.brackets += 1,
            ']' => self.brackets = self.brackets.saturating_sub(1),
            '⟨' => self.angles += 1,
            '⟩' => self.angles = self.angles.saturating_sub(1),
            _ => {}
        }
    }

    fn is_zero(&self) -> bool {
        self.braces == 0 && self.parentheses == 0 && self.brackets == 0 && self.angles == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{graph_from_semantic, lower_document};
    use crate::DocumentKind;
    use crate::{
        BinaryOperator, Expression, LiteralExpression, PortDirection, PosetRef, Relation, SourceId,
        parse_document,
    };

    #[test]
    fn lowers_ports_instances_assignments_and_constraints() {
        let source = SourceId::new("rover.mcdp");
        let parsed = parse_document(
            source.clone(),
            "\
mcdp {
  provides velocity [m/s]
  requires cost [product(overall_cost: USD, total_mass: kg)]
  sub battery = instance `battery
  total = 10 USD
  required cost >= total
}
",
        );

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("document should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.ports.len(), 2);
        assert_eq!(model.ports[0].direction, PortDirection::Provides);
        assert!(matches!(model.ports[1].poset, PosetRef::Product(_)));
        assert_eq!(model.instances[0].name, "battery");
        assert_eq!(model.assignments[0].target, "total");
        assert!(matches!(
            model.assignments[0].expression_ast,
            Expression::Literal(LiteralExpression::Quantity(_))
        ));
        assert_eq!(model.constraints[0].relation, Relation::Geq);
        assert!(matches!(
            model.constraints[0].left_expr,
            Expression::Port(_)
        ));

        let graph = graph_from_semantic(Some("rover".to_owned()), &model);
        assert_eq!(graph.ports.len(), 2);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.constraints.len(), 2);
    }

    #[test]
    fn lowers_choose_entries() {
        let source = SourceId::new("Batteries.mcdp");
        let parsed = parse_document(
            source.clone(),
            "choose (NiMH: `Battery_NiMH, LFP: `Battery_LFP)",
        );

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("choose should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.choices.len(), 2);
        assert_eq!(model.choices[0].name, "NiMH");
        assert_eq!(model.choices[0].target, "Battery_NiMH");
    }

    #[test]
    fn lowers_intersection_entries() {
        let source = SourceId::new("Both.mcdp");
        let parsed = parse_document(
            source.clone(),
            "intersection (A: `Battery_A, B: `Battery_B)",
        );

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("intersection should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.kind, DocumentKind::Intersection);
        assert_eq!(model.choices.len(), 2);
        assert_eq!(model.choices[0].name, "A");
        assert_eq!(model.choices[0].target, "Battery_A");
    }

    #[test]
    fn lowers_implementation_resource() {
        let source = SourceId::new("battery.mcdp");
        let parsed = parse_document(
            source.clone(),
            "dp { implemented-by yaml resource(\"battery.yaml\") }",
        );

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("implementation should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.implementations[0].backend, "yaml");
        assert_eq!(
            model.implementations[0].resource.as_deref(),
            Some("battery.yaml")
        );
    }

    #[test]
    fn lowers_port_without_space_before_poset() {
        let source = SourceId::new("catalog.mcdp");
        let parsed = parse_document(source.clone(), "catalog { requires timebudget[s] }");

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("port should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.ports[0].name, "timebudget");
    }

    #[test]
    fn lowers_catalog_records() {
        let source = SourceId::new("catalog.mcdp");
        let parsed = parse_document(source.clone(), "catalog { 0.5 m <-| robot_1 |-> 10 USD }");

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("catalog should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.catalog_records[0].implementation, "robot_1");
        assert_eq!(model.catalog_records[0].provided_values.len(), 1);
        assert_eq!(model.catalog_records[0].required_values.len(), 1);
    }

    #[test]
    fn lowers_constant_assignments_and_expression_ast() {
        let source = SourceId::new("robot.mcdp");
        let parsed = parse_document(
            source.clone(),
            "mcdp { constant v_to_cost = 25 USD * s / m\nrequired cost >= provided mass * v_to_cost }",
        );

        let (model, diagnostics) = lower_document(source, &parsed);
        let model = model.expect("constant should lower");

        assert!(diagnostics.is_empty());
        assert_eq!(model.assignments[0].target, "v_to_cost");
        assert!(matches!(
            model.constraints[0].right_expr,
            Expression::Binary {
                operator: BinaryOperator::Mul,
                ..
            }
        ));
    }

    #[test]
    fn reports_duplicate_ports() {
        let source = SourceId::new("bad.mcdp");
        let parsed = parse_document(source.clone(), "mcdp { provides x [kg]\nprovides x [kg] }");

        let (_model, diagnostics) = lower_document(source, &parsed);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "compiler.duplicate-port")
        );
    }

    #[test]
    fn reports_duplicate_names_across_ports_and_instances() {
        let source = SourceId::new("bad.mcdp");
        let parsed = parse_document(
            source.clone(),
            "\
dp {
  provides name [Nat]
  requires name [J]

  sub name = instance `model

  implemented-by yaml resource(\"test\")
}
",
        );

        let (_model, diagnostics) = lower_document(source, &parsed);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "compiler.duplicate-name"),
            "{diagnostics:?}"
        );
    }
}
