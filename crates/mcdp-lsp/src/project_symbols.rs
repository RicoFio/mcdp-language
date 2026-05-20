//! Project-level symbol indexing for future semantic editor features.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use mcdp_language::{
    DocumentKind, PortDirection as LanguagePortDirection, PosetRef, SemanticModel, Severity,
    SourceId, Statement, StatementKind, TextRange, Token, TokenKind, UnitExpression, lex,
    lower_document, normalize_unit_text, parse_document, parse_unit_expression_text,
};
use tower_lsp::lsp_types::Url;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectSymbolIndex {
    pub(crate) documents: HashMap<Url, DocumentSymbols>,
}

impl ProjectSymbolIndex {
    pub(crate) fn for_uri(uri: &Url, open_documents: &HashMap<Url, String>) -> Self {
        let mut sources = project_sources(uri);
        for (document_uri, source) in open_documents {
            sources.insert(document_uri.clone(), source.clone());
        }

        let mut index = Self::default();
        for (document_uri, source) in sources {
            index.upsert(document_uri, &source);
        }
        index
    }

    fn upsert(&mut self, uri: Url, source: &str) {
        let symbols = DocumentSymbols::parse(uri.clone(), source);
        self.documents.insert(uri, symbols);
    }

    pub(crate) fn semantic_diagnostics(&self, uri: &Url) -> Vec<ProjectDiagnostic> {
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mut diagnostics = Vec::new();
        self.instance_reference_diagnostics(document, &mut diagnostics);
        self.undefined_unit_diagnostics(document, &mut diagnostics);
        self.unit_agreement_diagnostics(document, &mut diagnostics);
        diagnostics
    }

    pub(crate) fn diagnostic_refresh_uris<'a>(
        &self,
        changed_uri: &Url,
        open_uris: impl IntoIterator<Item = &'a Url>,
    ) -> Vec<Url> {
        let changed_model = model_name(changed_uri);
        let changed_poset = (document_extension(changed_uri).as_deref() == Some("mcdp_poset"))
            .then(|| changed_model.clone())
            .flatten();

        let mut seen = BTreeSet::new();
        let mut refresh_uris = Vec::new();
        for uri in open_uris {
            if uri == changed_uri
                || self.document_depends_on(uri, changed_model.as_deref(), changed_poset.as_deref())
            {
                let key = uri.as_str().to_owned();
                if seen.insert(key) {
                    refresh_uris.push(uri.clone());
                }
            }
        }
        refresh_uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        refresh_uris
    }

    pub(crate) fn model_completion_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.documents.keys().filter_map(model_name).collect();
        names.sort();
        names.dedup();
        names
    }

    pub(crate) fn instance_completion_names(&self, uri: &Url) -> Vec<String> {
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mut names: Vec<String> = document
            .instances
            .iter()
            .map(|instance| instance.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub(crate) fn instance_port_completions(
        &self,
        uri: &Url,
        instance_name: &str,
        direction: PortDirection,
    ) -> Vec<PortSymbol> {
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let Some(instance) = document
            .instances
            .iter()
            .find(|instance| instance.name == instance_name)
        else {
            return Vec::new();
        };
        let Some(model) = instance.model.as_ref() else {
            return Vec::new();
        };
        let Some(target_document) = self.model_document(&model.name) else {
            return Vec::new();
        };
        let mut ports = match direction {
            PortDirection::Provided => target_document.provides.clone(),
            PortDirection::Required => target_document.requires.clone(),
        };
        ports.sort_by(|left, right| left.name.cmp(&right.name));
        ports.dedup_by(|left, right| left.name == right.name);
        ports
    }

    pub(crate) fn symbol_at(&self, uri: &Url, offset: usize) -> Option<ResolvedSymbol> {
        let document = self.documents.get(uri)?;

        if let Some(reference) = document
            .model_references
            .iter()
            .find(|reference| contains_offset(reference.reference_range, offset))
        {
            let target_document = self.model_document(&reference.name)?;
            return Some(ResolvedSymbol {
                target: SymbolTarget::Model {
                    uri: target_document.uri.clone(),
                },
                occurrence: SymbolOccurrence {
                    uri: uri.clone(),
                    range: reference.name_range,
                    kind: OccurrenceKind::Text,
                },
                context: SymbolContext::ModelReference {
                    name: reference.name.clone(),
                },
            });
        }

        if let Some(reference) = document.instance_scoped_reference_at(offset) {
            let (target_document, port) =
                self.resolve_instance_scoped_reference(document, &reference)?;
            return Some(ResolvedSymbol {
                target: SymbolTarget::Port {
                    uri: target_document.uri.clone(),
                    range: port.name_range,
                    direction: reference.direction,
                    name: port.name.clone(),
                },
                occurrence: SymbolOccurrence {
                    uri: uri.clone(),
                    range: reference.port_range,
                    kind: OccurrenceKind::Read,
                },
                context: SymbolContext::InstanceScopedVariable {
                    port_name: reference.port_name,
                    instance_name: reference.instance_name,
                    model_name: model_name(&target_document.uri)
                        .unwrap_or_else(|| target_document.uri.to_string()),
                    direction: reference.direction,
                },
            });
        }

        let (port, direction) = document.port_at(offset)?;
        Some(ResolvedSymbol {
            target: SymbolTarget::Port {
                uri: document.uri.clone(),
                range: port.name_range,
                direction,
                name: port.name.clone(),
            },
            occurrence: SymbolOccurrence {
                uri: uri.clone(),
                range: port.name_range,
                kind: OccurrenceKind::Write,
            },
            context: SymbolContext::PortDeclaration {
                name: port.name.clone(),
                direction,
            },
        })
    }

    pub(crate) fn definition_at(&self, uri: &Url, offset: usize) -> Option<DefinitionTarget> {
        let symbol = self.symbol_at(uri, offset)?;
        self.definition_for_target(&symbol.target)
    }

    pub(crate) fn document_occurrences_at(
        &self,
        uri: &Url,
        offset: usize,
    ) -> Option<Vec<SymbolOccurrence>> {
        let symbol = self.symbol_at(uri, offset)?;
        let mut occurrences = self.references_for_symbol(&symbol, true);
        occurrences.retain(|occurrence| &occurrence.uri == uri);
        Some(occurrences)
    }

    pub(crate) fn references_at(
        &self,
        uri: &Url,
        offset: usize,
        include_declaration: bool,
    ) -> Option<Vec<SymbolOccurrence>> {
        let symbol = self.symbol_at(uri, offset)?;
        Some(self.references_for_symbol(&symbol, include_declaration))
    }

    pub(crate) fn hover_at(&self, uri: &Url, offset: usize) -> Option<HoverInfo> {
        if let Some(symbol) = self.symbol_at(uri, offset) {
            return Some(self.hover_for_symbol(&symbol));
        }

        self.documents
            .get(uri)
            .and_then(|document| document.term_hover_at(offset))
    }

    fn references_for_symbol(
        &self,
        symbol: &ResolvedSymbol,
        include_declaration: bool,
    ) -> Vec<SymbolOccurrence> {
        let mut occurrences = Vec::new();
        if include_declaration && let Some(definition) = self.definition_for_target(&symbol.target)
        {
            occurrences.push(SymbolOccurrence {
                uri: definition.uri,
                range: definition.range,
                kind: OccurrenceKind::Write,
            });
        }

        match &symbol.target {
            SymbolTarget::Model { uri } => self.model_reference_occurrences(uri, &mut occurrences),
            SymbolTarget::Port { .. } => {
                self.instance_scoped_port_occurrences(&symbol.target, &mut occurrences);
            }
        }

        occurrences.sort_by(|left, right| {
            left.uri
                .as_str()
                .cmp(right.uri.as_str())
                .then(left.range.start.cmp(&right.range.start))
                .then(left.range.end.cmp(&right.range.end))
        });
        occurrences.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);
        occurrences
    }

    fn definition_for_target(&self, target: &SymbolTarget) -> Option<DefinitionTarget> {
        match target {
            SymbolTarget::Model { uri } => {
                let document = self.documents.get(uri)?;
                Some(DefinitionTarget {
                    uri: document.uri.clone(),
                    range: document.definition_range,
                })
            }
            SymbolTarget::Port { uri, range, .. } => Some(DefinitionTarget {
                uri: uri.clone(),
                range: *range,
            }),
        }
    }

    fn model_reference_occurrences(
        &self,
        target_uri: &Url,
        occurrences: &mut Vec<SymbolOccurrence>,
    ) {
        for document in self.documents.values() {
            for reference in &document.model_references {
                let Some(model_document) = self.model_document(&reference.name) else {
                    continue;
                };
                if &model_document.uri == target_uri {
                    occurrences.push(SymbolOccurrence {
                        uri: document.uri.clone(),
                        range: reference.name_range,
                        kind: OccurrenceKind::Text,
                    });
                }
            }
        }
    }

    fn instance_scoped_port_occurrences(
        &self,
        target: &SymbolTarget,
        occurrences: &mut Vec<SymbolOccurrence>,
    ) {
        for document in self.documents.values() {
            for reference in document.instance_scoped_references() {
                let Some((target_document, port)) =
                    self.resolve_instance_scoped_reference(document, &reference)
                else {
                    continue;
                };
                let candidate = SymbolTarget::Port {
                    uri: target_document.uri.clone(),
                    range: port.name_range,
                    direction: reference.direction,
                    name: port.name.clone(),
                };
                if &candidate == target {
                    occurrences.push(SymbolOccurrence {
                        uri: document.uri.clone(),
                        range: reference.port_range,
                        kind: OccurrenceKind::Read,
                    });
                }
            }
        }
    }

    fn resolve_instance_scoped_reference<'a>(
        &'a self,
        document: &'a DocumentSymbols,
        reference: &InstanceScopedReference,
    ) -> Option<(&'a DocumentSymbols, &'a PortSymbol)> {
        let instance = document
            .instances
            .iter()
            .find(|instance| instance.name == reference.instance_name)?;
        let model = instance.model.as_ref()?;
        let target_document = self.model_document(&model.name)?;
        let port = target_document.port_named(reference.direction, &reference.port_name)?;
        Some((target_document, port))
    }

    fn hover_for_symbol(&self, symbol: &ResolvedSymbol) -> HoverInfo {
        let contents = match &symbol.context {
            SymbolContext::ModelReference { name } => {
                match self.definition_for_target(&symbol.target) {
                    Some(definition) => {
                        let target = model_name(&definition.uri)
                            .unwrap_or_else(|| definition.uri.to_string());
                        format!(
                            "**Model reference** `{name}`\n\nResolves to the `{target}` MCDPL document."
                        )
                    }
                    None => format!("**Model reference** `{name}`"),
                }
            }
            SymbolContext::InstanceScopedVariable {
                port_name,
                instance_name,
                model_name,
                direction,
            } => format!(
                "**{} variable** `{port_name}`\n\nResolved through instance `{instance_name}` to `{model_name}`.",
                direction.hover_label()
            ),
            SymbolContext::PortDeclaration { name, direction } => format!(
                "**{} declaration** `{name}`\n\nDeclares a {} on this model.",
                direction.hover_label(),
                direction.role_description()
            ),
        };

        HoverInfo {
            range: symbol.occurrence.range,
            contents,
        }
    }

    fn model_document(&self, name: &str) -> Option<&DocumentSymbols> {
        self.documents
            .values()
            .filter(|document| model_name(&document.uri).as_deref() == Some(name))
            .min_by_key(|document| document_priority(&document.uri))
    }

    fn poset_document(&self, name: &str) -> Option<&DocumentSymbols> {
        self.documents.values().find(|document| {
            model_name(&document.uri).as_deref() == Some(name)
                && document_extension(&document.uri).as_deref() == Some("mcdp_poset")
        })
    }

    fn document_depends_on(
        &self,
        uri: &Url,
        changed_model: Option<&str>,
        changed_poset: Option<&str>,
    ) -> bool {
        let Some(document) = self.documents.get(uri) else {
            return false;
        };
        if let Some(changed_model) = changed_model
            && document
                .model_references
                .iter()
                .any(|reference| reference.name == changed_model)
        {
            return true;
        }
        if let Some(changed_poset) = changed_poset {
            return document.declared_units.iter().any(|unit| {
                unit_atoms(&unit.name)
                    .iter()
                    .any(|atom| atom.name == changed_poset)
            });
        }
        false
    }

    fn undefined_unit_diagnostics(
        &self,
        document: &DocumentSymbols,
        diagnostics: &mut Vec<ProjectDiagnostic>,
    ) {
        let mut reported = BTreeSet::new();
        for unit in &document.declared_units {
            for atom in unit_atoms(&unit.name) {
                if is_base_unit_atom(&atom.name) {
                    continue;
                }
                if self.poset_document(&atom.name).is_some() {
                    continue;
                }
                if !reported.insert((unit.range.start, atom.name.clone())) {
                    continue;
                }

                let (code, message, help) = if atom.named {
                    (
                        "lsp.undefined-poset",
                        format!("named poset `{}` was not found", atom.name),
                        "Named posets are resolved as `.mcdp_poset` files in the current MCDPL library.",
                    )
                } else {
                    (
                        "lsp.undefined-unit",
                        format!(
                            "unit `{}` is not a known base unit or project poset",
                            atom.name
                        ),
                        "Define a matching `.mcdp_poset` file or use a built-in unit/poset such as Nat, Bool, USD, kg, m, or s.",
                    )
                };
                diagnostics.push(ProjectDiagnostic {
                    code: code.to_owned(),
                    message,
                    help: Some(help.to_owned()),
                    severity: Severity::Warning,
                    range: unit.range,
                });
            }
        }
    }

    fn instance_reference_diagnostics(
        &self,
        document: &DocumentSymbols,
        diagnostics: &mut Vec<ProjectDiagnostic>,
    ) {
        let mut reported = BTreeSet::new();
        for reference in document.instance_scoped_references() {
            let Some(instance) = document
                .instances
                .iter()
                .find(|instance| instance.name == reference.instance_name)
            else {
                if reported.insert((
                    "lsp.undefined-instance",
                    reference.instance_range.start,
                    reference.instance_name.clone(),
                )) {
                    diagnostics.push(ProjectDiagnostic {
                        code: "lsp.undefined-instance".to_owned(),
                        message: format!(
                            "instance `{}` is not declared in this document",
                            reference.instance_name
                        ),
                        help: Some(format!(
                            "Add a matching `sub {} = instance ...` declaration or update the `by {}` reference.",
                            reference.instance_name, reference.instance_name
                        )),
                        severity: Severity::Error,
                        range: reference.instance_range,
                    });
                }
                continue;
            };

            let Some(model) = instance.model.as_ref() else {
                continue;
            };
            let Some(target_document) = self.model_document(&model.name) else {
                if reported.insert((
                    "lsp.undefined-model",
                    model.name_range.start,
                    model.name.clone(),
                )) {
                    diagnostics.push(ProjectDiagnostic {
                        code: "lsp.undefined-model".to_owned(),
                        message: format!(
                            "model `{}` for instance `{}` was not found",
                            model.name, instance.name
                        ),
                        help: Some(
                            "Add the referenced `.mcdp`, `.mcdp_interface`, or `.mcdp_template` file to this MCDPL library."
                                .to_owned(),
                        ),
                        severity: Severity::Error,
                        range: model.name_range,
                    });
                }
                continue;
            };

            if target_document
                .port_named(reference.direction, &reference.port_name)
                .is_some()
            {
                continue;
            }

            let code = match reference.direction {
                PortDirection::Provided => "lsp.undefined-provided-port",
                PortDirection::Required => "lsp.undefined-required-port",
            };
            if reported.insert((
                code,
                reference.port_range.start,
                reference.port_name.clone(),
            )) {
                diagnostics.push(ProjectDiagnostic {
                    code: code.to_owned(),
                    message: format!(
                        "{} port `{}` was not found on instance `{}`",
                        reference.direction.diagnostic_label(),
                        reference.port_name,
                        reference.instance_name
                    ),
                    help: Some(format!(
                        "Instance `{}` resolves to `{}`; check that document's {} declarations or update this reference.",
                        reference.instance_name,
                        model.name,
                        reference.direction.declaration_keyword()
                    )),
                    severity: Severity::Error,
                    range: reference.port_range,
                });
            }
        }
    }

    fn unit_agreement_diagnostics(
        &self,
        document: &DocumentSymbols,
        diagnostics: &mut Vec<ProjectDiagnostic>,
    ) {
        for relation in &document.relations {
            let Some(left) = self.expression_unit(document, relation.left_range, diagnostics)
            else {
                continue;
            };
            let Some(right) = self.expression_unit(document, relation.right_range, diagnostics)
            else {
                continue;
            };
            if !left.scalar || !right.scalar {
                continue;
            }
            if left.unit == right.unit {
                continue;
            }
            diagnostics.push(ProjectDiagnostic {
                code: "lsp.unit-mismatch".to_owned(),
                message: format!(
                    "unit mismatch: `{}` has unit `{}`, expected `{}`",
                    right.label,
                    right.unit.label(),
                    left.unit.label(),
                ),
                help: Some(format!(
                    "This relation compares values against `{}`; both sides should have compatible units.",
                    left.label
                )),
                severity: Severity::Error,
                range: right.range,
            });
        }
    }

    fn expression_unit(
        &self,
        document: &DocumentSymbols,
        range: TextRange,
        diagnostics: &mut Vec<ProjectDiagnostic>,
    ) -> Option<ExpressionUnit> {
        let tokens: Vec<Token> = lex(&document.source)
            .into_iter()
            .filter(|token| {
                !is_trivia(token.kind)
                    && token.range.start >= range.start
                    && token.range.end <= range.end
            })
            .collect();
        if tokens.is_empty() {
            return None;
        }

        let mut parser = ExpressionUnitParser::new(self, document, &tokens, diagnostics);
        parser.parse_expression()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentSymbols {
    pub(crate) uri: Url,
    pub(crate) source: String,
    pub(crate) kind: Option<DocumentKind>,
    pub(crate) definition_range: TextRange,
    pub(crate) provides: Vec<PortSymbol>,
    pub(crate) requires: Vec<PortSymbol>,
    pub(crate) assignments: Vec<AssignmentSymbol>,
    pub(crate) instances: Vec<InstanceBinding>,
    pub(crate) model_references: Vec<ModelReference>,
    pub(crate) resource_bindings: Vec<ResourceBinding>,
    pub(crate) declared_units: Vec<DeclaredUnit>,
    pub(crate) relations: Vec<RelationConstraint>,
}

impl DocumentSymbols {
    pub(crate) fn parse(uri: Url, source: &str) -> Self {
        let source_id = SourceId::new(uri.as_str());
        let parsed = parse_document(source_id.clone(), source);
        let (semantic_model, _) = lower_document(source_id, &parsed);
        let definition_range = parsed
            .tokens
            .iter()
            .find(|token| !is_trivia(token.kind))
            .map_or_else(TextRange::default, |token| token.range);
        let mut symbols = Self {
            uri,
            source: source.to_owned(),
            kind: parsed.kind,
            definition_range,
            provides: Vec::new(),
            requires: Vec::new(),
            assignments: Vec::new(),
            instances: Vec::new(),
            model_references: model_references(&parsed.tokens),
            resource_bindings: Vec::new(),
            declared_units: Vec::new(),
            relations: Vec::new(),
        };

        let Some(syntax) = parsed.syntax else {
            return symbols;
        };

        for statement in syntax.body.statements() {
            let statement_tokens = statement_tokens(&parsed.tokens, statement);
            match statement.kind {
                StatementKind::Provides => {
                    if let Some(mut port) = port_symbol(source, statement, &statement_tokens) {
                        port.poset = semantic_port_poset(
                            semantic_model.as_ref(),
                            PortDirection::Provided,
                            &port.name,
                        );
                        if let Some(unit) = port.unit.clone() {
                            symbols.declared_units.push(DeclaredUnit {
                                name: unit.text,
                                range: unit.range,
                                declaration_range: statement.range,
                            });
                        }
                        symbols.provides.push(port);
                    }
                }
                StatementKind::Requires => {
                    if let Some(mut port) = port_symbol(source, statement, &statement_tokens) {
                        port.poset = semantic_port_poset(
                            semantic_model.as_ref(),
                            PortDirection::Required,
                            &port.name,
                        );
                        if let Some(unit) = port.unit.clone() {
                            symbols.declared_units.push(DeclaredUnit {
                                name: unit.text,
                                range: unit.range,
                                declaration_range: statement.range,
                            });
                        }
                        symbols.requires.push(port);
                    }
                }
                StatementKind::Instance => {
                    if let Some(instance) = instance_binding(statement, &statement_tokens) {
                        symbols.instances.push(instance);
                    }
                }
                StatementKind::ImplementedBy => {
                    if let Some(resource) = resource_binding(statement, &statement_tokens) {
                        symbols.resource_bindings.push(resource);
                    }
                }
                StatementKind::Constraint => {
                    if let Some(relation) =
                        relation_constraint(source, statement, &statement_tokens)
                    {
                        symbols.relations.push(relation);
                    }
                }
                StatementKind::Assignment => {
                    if let Some(assignment) =
                        assignment_symbol(source, statement, &statement_tokens)
                    {
                        if let Some(unit) = assignment.unit.clone() {
                            symbols.declared_units.push(DeclaredUnit {
                                name: unit.text,
                                range: unit.range,
                                declaration_range: statement.range,
                            });
                        }
                        symbols.assignments.push(assignment);
                    }
                }
                StatementKind::Implements
                | StatementKind::Import
                | StatementKind::CatalogRecord
                | StatementKind::BareExpression => {}
            }
        }

        symbols
    }

    fn instance_scoped_reference_at(&self, offset: usize) -> Option<InstanceScopedReference> {
        let tokens: Vec<Token> = lex(&self.source)
            .into_iter()
            .filter(|token| !is_trivia(token.kind))
            .collect();
        let index = tokens
            .iter()
            .position(|token| is_symbol_name(token) && contains_offset(token.range, offset))?;
        instance_scoped_reference_at_tokens(&tokens, index)
    }

    pub(crate) fn instance_scoped_references(&self) -> Vec<InstanceScopedReference> {
        let tokens: Vec<Token> = lex(&self.source)
            .into_iter()
            .filter(|token| !is_trivia(token.kind))
            .collect();

        tokens
            .iter()
            .enumerate()
            .filter_map(|(index, token)| {
                is_symbol_name(token)
                    .then(|| instance_scoped_reference_at_tokens(&tokens, index))?
            })
            .collect()
    }

    fn port_at(&self, offset: usize) -> Option<(&PortSymbol, PortDirection)> {
        if let Some(port) = self
            .provides
            .iter()
            .find(|port| contains_offset(port.name_range, offset))
        {
            return Some((port, PortDirection::Provided));
        }

        self.requires
            .iter()
            .find(|port| contains_offset(port.name_range, offset))
            .map(|port| (port, PortDirection::Required))
    }

    fn port_named(&self, direction: PortDirection, name: &str) -> Option<&PortSymbol> {
        match direction {
            PortDirection::Provided => self.provides.iter().find(|port| port.name == name),
            PortDirection::Required => self.requires.iter().find(|port| port.name == name),
        }
    }

    fn assignment_named(&self, name: &str) -> Option<&AssignmentSymbol> {
        self.assignments
            .iter()
            .find(|assignment| assignment.name == name)
    }

    fn term_hover_at(&self, offset: usize) -> Option<HoverInfo> {
        let token = lex(&self.source)
            .into_iter()
            .find(|token| contains_offset(token.range, offset))?;
        let contents = term_hover_text(&token.text)
            .map(str::to_owned)
            .or_else(|| primitive_poset_hover_text(&token.text))?;

        Some(HoverInfo {
            range: token.range,
            contents,
        })
    }
}

fn instance_scoped_reference_at_tokens(
    tokens: &[Token],
    index: usize,
) -> Option<InstanceScopedReference> {
    let direction = match tokens.get(index + 1).map(|token| token.text.as_str()) {
        Some("provided") => PortDirection::Provided,
        Some("required") => PortDirection::Required,
        _ => return None,
    };
    if tokens.get(index + 2).map(|token| token.text.as_str()) != Some("by") {
        return None;
    }
    let instance = tokens.get(index + 3)?;
    if !is_symbol_name(instance) {
        return None;
    }

    Some(InstanceScopedReference {
        port_name: tokens[index].text.clone(),
        port_range: tokens[index].range,
        instance_name: instance.text.clone(),
        instance_range: instance.range,
        direction,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DefinitionTarget {
    pub(crate) uri: Url,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedSymbol {
    target: SymbolTarget,
    occurrence: SymbolOccurrence,
    context: SymbolContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SymbolTarget {
    Model {
        uri: Url,
    },
    Port {
        uri: Url,
        range: TextRange,
        direction: PortDirection,
        name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SymbolContext {
    ModelReference {
        name: String,
    },
    InstanceScopedVariable {
        port_name: String,
        instance_name: String,
        model_name: String,
        direction: PortDirection,
    },
    PortDeclaration {
        name: String,
        direction: PortDirection,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolOccurrence {
    pub(crate) uri: Url,
    pub(crate) range: TextRange,
    pub(crate) kind: OccurrenceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OccurrenceKind {
    Text,
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HoverInfo {
    pub(crate) range: TextRange,
    pub(crate) contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectDiagnostic {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) help: Option<String>,
    pub(crate) severity: Severity,
    pub(crate) range: TextRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrimitivePoset {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

pub(crate) fn primitive_posets() -> &'static [PrimitivePoset] {
    PRIMITIVE_POSETS
}

const PRIMITIVE_POSETS: &[PrimitivePoset] = &[
    PrimitivePoset {
        name: "Nat",
        description: "Natural numbers",
    },
    PrimitivePoset {
        name: "Bool",
        description: "Boolean truth values",
    },
    PrimitivePoset {
        name: "Real",
        description: "Real numbers",
    },
    PrimitivePoset {
        name: "Reals",
        description: "Real numbers",
    },
    PrimitivePoset {
        name: "N",
        description: "newtons, force",
    },
    PrimitivePoset {
        name: "Nm",
        description: "newton-meters, torque or energy",
    },
    PrimitivePoset {
        name: "J",
        description: "joules, energy",
    },
    PrimitivePoset {
        name: "W",
        description: "watts, power",
    },
    PrimitivePoset {
        name: "USD",
        description: "US dollars, cost",
    },
    PrimitivePoset {
        name: "kg",
        description: "kilograms, mass",
    },
    PrimitivePoset {
        name: "g",
        description: "grams, mass",
    },
    PrimitivePoset {
        name: "m",
        description: "meters, length",
    },
    PrimitivePoset {
        name: "s",
        description: "seconds, time",
    },
    PrimitivePoset {
        name: "rad",
        description: "radians, angle",
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortSymbol {
    pub(crate) name: String,
    pub(crate) name_range: TextRange,
    pub(crate) unit: Option<SymbolText>,
    pub(crate) poset: Option<PosetRef>,
    pub(crate) declaration_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AssignmentSymbol {
    pub(crate) name: String,
    pub(crate) name_range: TextRange,
    pub(crate) unit: Option<SymbolText>,
    pub(crate) expression_range: TextRange,
    pub(crate) declaration_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstanceBinding {
    pub(crate) name: String,
    pub(crate) name_range: TextRange,
    pub(crate) model: Option<ModelReference>,
    pub(crate) declaration_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelReference {
    pub(crate) name: String,
    pub(crate) name_range: TextRange,
    pub(crate) reference_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResourceBinding {
    pub(crate) path: String,
    pub(crate) path_range: TextRange,
    pub(crate) expression_range: TextRange,
    pub(crate) declaration_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeclaredUnit {
    pub(crate) name: String,
    pub(crate) range: TextRange,
    pub(crate) declaration_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelationConstraint {
    left_range: TextRange,
    relation_range: TextRange,
    right_range: TextRange,
    statement_range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolText {
    pub(crate) text: String,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UnitAtom {
    name: String,
    named: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct UnitFactors {
    factors: BTreeMap<String, i32>,
}

impl UnitFactors {
    fn from_text(text: &str) -> Self {
        let mut unit = Self::default();
        let mut operator = 1;
        let mut current = String::new();
        for ch in normalize_unit_text(text).chars() {
            match ch {
                '*' | '·' | '⋅' => {
                    unit.apply_factor(&current, operator);
                    current.clear();
                    operator = 1;
                }
                '/' => {
                    unit.apply_factor(&current, operator);
                    current.clear();
                    operator = -1;
                }
                _ => current.push(ch),
            }
        }
        unit.apply_factor(&current, operator);
        unit
    }

    fn from_optional_symbol_text(unit: Option<&SymbolText>) -> Self {
        unit.map_or_else(Self::one, |unit| Self::from_text(&unit.text))
    }

    fn one() -> Self {
        Self::default()
    }

    fn multiply(&self, right: &Self) -> Self {
        let mut product = self.clone();
        product.extend(right, 1);
        product
    }

    fn divide(&self, right: &Self) -> Self {
        let mut quotient = self.clone();
        quotient.extend(right, -1);
        quotient
    }

    fn label(&self) -> String {
        let numerator = self.factor_labels(true);
        let denominator = self.factor_labels(false);
        match (numerator.is_empty(), denominator.is_empty()) {
            (true, true) => "unitless".to_owned(),
            (false, true) => numerator.join("*"),
            (true, false) => format!("1/{}", denominator.join("/")),
            (false, false) => format!("{}/{}", numerator.join("*"), denominator.join("/")),
        }
    }

    fn extend(&mut self, right: &Self, sign: i32) {
        for (factor, exponent) in &right.factors {
            self.add_factor(factor, sign * exponent);
        }
    }

    fn apply_factor(&mut self, raw: &str, sign: i32) {
        let token = raw.trim();
        if token.is_empty()
            || matches!(token, "1" | "dimensionless" | "unitless" | "Reals" | "Real")
        {
            return;
        }

        let (base, exponent) = unit_base_and_exponent(token);
        for (factor, base_exponent) in dimension_unit_atom_factors(&base) {
            self.add_factor(&factor, sign * exponent * base_exponent);
        }
    }

    fn add_factor(&mut self, factor: &str, exponent: i32) {
        if factor.is_empty() || exponent == 0 {
            return;
        }
        let total = self.factors.entry(factor.to_owned()).or_default();
        *total += exponent;
        if *total == 0 {
            self.factors.remove(factor);
        }
    }

    fn factor_labels(&self, positive: bool) -> Vec<String> {
        self.factors
            .iter()
            .filter_map(|(unit, exponent)| {
                let exponent = if positive { *exponent } else { -*exponent };
                (exponent > 0).then(|| unit_factor_label(unit, exponent))
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpressionUnit {
    label: String,
    unit: UnitFactors,
    unit_text: Option<String>,
    poset: Option<PosetRef>,
    scalar: bool,
    range: TextRange,
}

struct ExpressionUnitParser<'index, 'document, 'tokens, 'diagnostics> {
    index: &'index ProjectSymbolIndex,
    document: &'document DocumentSymbols,
    tokens: &'tokens [Token],
    position: usize,
    diagnostics: &'diagnostics mut Vec<ProjectDiagnostic>,
}

impl<'index, 'document, 'tokens, 'diagnostics>
    ExpressionUnitParser<'index, 'document, 'tokens, 'diagnostics>
{
    fn new(
        index: &'index ProjectSymbolIndex,
        document: &'document DocumentSymbols,
        tokens: &'tokens [Token],
        diagnostics: &'diagnostics mut Vec<ProjectDiagnostic>,
    ) -> Self {
        Self {
            index,
            document,
            tokens,
            position: 0,
            diagnostics,
        }
    }

    fn parse_expression(&mut self) -> Option<ExpressionUnit> {
        let expression = self.parse_additive()?;
        (self.position == self.tokens.len()).then_some(expression)
    }

    fn parse_additive(&mut self) -> Option<ExpressionUnit> {
        let mut expression = self.parse_multiplicative()?;
        loop {
            let operator = if self.consume_text("+") {
                Some("+")
            } else if self.consume_text("-") {
                Some("-")
            } else {
                None
            };
            let Some(operator) = operator else {
                break;
            };
            let right = self.parse_multiplicative()?;
            if !expression.scalar || !right.scalar {
                return None;
            }
            if expression.unit != right.unit {
                self.push_additive_mismatch(&expression, &right, operator);
                return None;
            }
            expression = ExpressionUnit {
                label: expression.label,
                unit: expression.unit,
                unit_text: expression.unit_text,
                poset: expression.poset,
                scalar: expression.scalar,
                range: merge_range(expression.range, right.range),
            };
        }
        Some(expression)
    }

    fn parse_multiplicative(&mut self) -> Option<ExpressionUnit> {
        let mut expression = self.parse_unary()?;
        loop {
            if self.consume_text("*") || self.consume_text("·") || self.consume_text("⋅") {
                let right = self.parse_unary()?;
                if !expression.scalar || !right.scalar {
                    return None;
                }
                expression = ExpressionUnit {
                    label: "expression".to_owned(),
                    unit: expression.unit.multiply(&right.unit),
                    unit_text: None,
                    poset: None,
                    scalar: true,
                    range: merge_range(expression.range, right.range),
                };
            } else if self.consume_text("/") {
                let right = self.parse_unary()?;
                if !expression.scalar || !right.scalar {
                    return None;
                }
                expression = ExpressionUnit {
                    label: "expression".to_owned(),
                    unit: expression.unit.divide(&right.unit),
                    unit_text: None,
                    poset: None,
                    scalar: true,
                    range: merge_range(expression.range, right.range),
                };
            } else {
                break;
            }
        }
        Some(expression)
    }

    fn parse_unary(&mut self) -> Option<ExpressionUnit> {
        if let Some(operator) = self.consume_token("-") {
            let expression = self.parse_unary()?;
            return Some(ExpressionUnit {
                label: expression.label,
                unit: expression.unit,
                unit_text: expression.unit_text,
                poset: expression.poset,
                scalar: expression.scalar,
                range: merge_range(operator.range, expression.range),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<ExpressionUnit> {
        let mut expression = self.parse_primary()?;
        while self.consume_text(".") {
            let field = self.consume_word()?;
            let field_poset = expression.field_poset(&field.text)?;
            let field_unit = poset_unit_text(&field_poset);
            expression = ExpressionUnit {
                label: field.text,
                unit: field_unit
                    .as_deref()
                    .map_or_else(UnitFactors::one, UnitFactors::from_text),
                unit_text: field_unit,
                poset: Some(field_poset.clone()),
                scalar: !matches!(field_poset, PosetRef::Product(_)),
                range: field.range,
            };
        }
        Some(expression)
    }

    fn parse_primary(&mut self) -> Option<ExpressionUnit> {
        if let Some(open) = self.consume_token("(") {
            let expression = self.parse_additive()?;
            let close = self.consume_token(")")?;
            return Some(ExpressionUnit {
                label: expression.label,
                unit: expression.unit,
                unit_text: expression.unit_text,
                poset: expression.poset,
                scalar: expression.scalar,
                range: TextRange::new(open.range.start, close.range.end),
            });
        }

        if self.peek_text("provided") || self.peek_text("required") {
            return self.parse_prefix_port_reference();
        }

        if self.peek_text("sum") {
            return self.parse_aggregate();
        }

        if self.peek_kind(TokenKind::Number) {
            return self.parse_quantity();
        }

        if self.peek_text("`") {
            self.skip_symbol_literal();
            return None;
        }

        if self.peek_word() {
            return self.parse_word_primary();
        }

        None
    }

    fn parse_prefix_port_reference(&mut self) -> Option<ExpressionUnit> {
        let direction = self.consume_direction()?;
        let port = self.consume_word()?;
        if self.consume_text("by") {
            let instance = self.consume_instance_selector()?;
            if instance.text == "*" {
                return None;
            }
            let reference = InstanceScopedReference {
                port_name: port.text.clone(),
                port_range: port.range,
                instance_name: instance.text,
                instance_range: instance.range,
                direction,
            };
            return self.instance_scoped_unit(&reference);
        }

        self.local_port_unit(direction, &port)
    }

    fn parse_word_primary(&mut self) -> Option<ExpressionUnit> {
        let name = self.consume_word()?;
        if self.consume_text("(") {
            self.skip_until_balanced_close("(", ")");
            return None;
        }

        if let Some(direction) = self.consume_direction() {
            if !self.consume_text("by") {
                return None;
            }
            let instance = self.consume_instance_selector()?;
            if instance.text == "*" {
                return None;
            }
            let reference = InstanceScopedReference {
                port_name: name.text.clone(),
                port_range: name.range,
                instance_name: instance.text,
                instance_range: instance.range,
                direction,
            };
            return self.instance_scoped_unit(&reference);
        }

        self.assignment_unit(&name)
    }

    fn parse_aggregate(&mut self) -> Option<ExpressionUnit> {
        self.consume_text("sum");
        let port = self.consume_word()?;
        let direction = self.consume_direction()?;
        if !self.consume_text("by") {
            return None;
        }
        let instance = self.consume_instance_selector()?;
        if instance.text == "*" {
            return self.aggregate_wildcard_unit(direction, &port);
        }

        let reference = InstanceScopedReference {
            port_name: port.text.clone(),
            port_range: port.range,
            instance_name: instance.text,
            instance_range: instance.range,
            direction,
        };
        self.instance_scoped_unit(&reference)
    }

    fn parse_quantity(&mut self) -> Option<ExpressionUnit> {
        let number = self.consume_token_kind(TokenKind::Number)?;
        if let Some((unit, range)) = self.consume_bracket_unit() {
            return Some(ExpressionUnit::from_unit(
                number.text,
                unit,
                TextRange::new(number.range.start, range.end),
            ));
        }
        if let Some((unit, range)) = self.consume_trailing_unit() {
            return Some(ExpressionUnit::from_unit(
                number.text,
                unit,
                TextRange::new(number.range.start, range.end),
            ));
        }

        Some(ExpressionUnit {
            label: number.text,
            unit: UnitFactors::one(),
            unit_text: None,
            poset: None,
            scalar: true,
            range: number.range,
        })
    }

    fn local_port_unit(&self, direction: PortDirection, port: &Token) -> Option<ExpressionUnit> {
        let port_symbol = self.document.port_named(direction, &port.text)?;
        Some(ExpressionUnit::from_port_symbol(
            port.text.clone(),
            port.range,
            port_symbol,
        ))
    }

    fn instance_scoped_unit(&self, reference: &InstanceScopedReference) -> Option<ExpressionUnit> {
        let (_, port) = self
            .index
            .resolve_instance_scoped_reference(self.document, reference)?;
        Some(ExpressionUnit::from_port_symbol(
            reference.port_name.clone(),
            reference.port_range,
            port,
        ))
    }

    fn aggregate_wildcard_unit(
        &self,
        direction: PortDirection,
        port: &Token,
    ) -> Option<ExpressionUnit> {
        let mut resolved = self
            .document
            .instances
            .iter()
            .filter_map(|instance| {
                let model = instance.model.as_ref()?;
                let target_document = self.index.model_document(&model.name)?;
                target_document.port_named(direction, &port.text)
            })
            .map(|symbol| UnitFactors::from_optional_symbol_text(symbol.unit.as_ref()));
        let first = resolved.next()?;
        if resolved.all(|unit| unit == first) {
            Some(ExpressionUnit {
                label: port.text.clone(),
                unit: first,
                unit_text: None,
                poset: None,
                scalar: true,
                range: port.range,
            })
        } else {
            None
        }
    }

    fn assignment_unit(&mut self, name: &Token) -> Option<ExpressionUnit> {
        let assignment = self.document.assignment_named(&name.text)?;
        let unit = assignment.unit.as_ref()?;
        Some(ExpressionUnit::from_symbol_text(
            name.text.clone(),
            name.range,
            unit,
        ))
    }

    fn consume_bracket_unit(&mut self) -> Option<(String, TextRange)> {
        let open_index = self.position;
        let open = self.tokens.get(open_index)?;
        if open.text != "[" {
            return None;
        }
        let close_index = matching_close_tokens(self.tokens, open_index, "[", "]")?;
        let close = &self.tokens[close_index];
        let raw_range = TextRange::new(open.range.end, close.range.start);
        let range = trim_range(&self.document.source, raw_range);
        self.position = close_index + 1;
        if range.is_empty() {
            return None;
        }
        Some((
            self.document.source[range.start..range.end].to_owned(),
            close.range,
        ))
    }

    fn consume_trailing_unit(&mut self) -> Option<(String, TextRange)> {
        if !self.peek_unit_atom() {
            return None;
        }
        let start = self.position;
        self.position += 1;
        loop {
            if self.peek_text("^") || self.peek_text("^-") || self.peek_text("^+") {
                if !self.peek_offset_unit_atom(1) {
                    break;
                }
                self.position += 2;
                continue;
            }
            if matches!(self.peek_text_value(), Some("*" | "/" | "·" | "⋅"))
                && self.peek_offset_unit_atom(1)
            {
                self.position += 2;
                continue;
            }
            break;
        }

        let first = &self.tokens[start];
        let last = &self.tokens[self.position - 1];
        let text = self.document.source[first.range.start..last.range.end].to_owned();
        Some((text, TextRange::new(first.range.start, last.range.end)))
    }

    fn push_additive_mismatch(
        &mut self,
        left: &ExpressionUnit,
        right: &ExpressionUnit,
        operator: &str,
    ) {
        self.diagnostics.push(ProjectDiagnostic {
            code: "lsp.unit-mismatch".to_owned(),
            message: format!(
                "unit mismatch: `{}` has unit `{}`, but `{operator}` combines it with `{}` of unit `{}`",
                right.label,
                right.unit.label(),
                left.label,
                left.unit.label(),
            ),
            help: Some(
                "Additive expression terms must have compatible units before the relation is checked."
                    .to_owned(),
            ),
            severity: Severity::Error,
            range: right.range,
        });
    }

    fn skip_symbol_literal(&mut self) {
        self.consume_text("`");
        while self.position < self.tokens.len() {
            if self.consume_text(",") || self.consume_text(")") {
                self.position = self.position.saturating_sub(1);
                return;
            }
            if self.consume_text(":") {
                continue;
            }
            if self.peek_word() {
                self.position += 1;
                continue;
            }
            break;
        }
    }

    fn skip_until_balanced_close(&mut self, open_text: &str, close_text: &str) {
        let mut depth = 1usize;
        while self.position < self.tokens.len() {
            if self.peek_text(open_text) {
                depth += 1;
            } else if self.peek_text(close_text) {
                depth = depth.saturating_sub(1);
                self.position += 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            self.position += 1;
        }
    }

    fn consume_direction(&mut self) -> Option<PortDirection> {
        if self.consume_text("provided") {
            Some(PortDirection::Provided)
        } else if self.consume_text("required") {
            Some(PortDirection::Required)
        } else {
            None
        }
    }

    fn consume_word(&mut self) -> Option<Token> {
        if !self.peek_word() {
            return None;
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Some(token)
    }

    fn consume_instance_selector(&mut self) -> Option<Token> {
        if self.peek_text("*") {
            return self.consume_token("*");
        }
        self.consume_word()
    }

    fn consume_token(&mut self, text: &str) -> Option<Token> {
        if !self.peek_text(text) {
            return None;
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Some(token)
    }

    fn consume_token_kind(&mut self, kind: TokenKind) -> Option<Token> {
        if !self.peek_kind(kind) {
            return None;
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Some(token)
    }

    fn consume_text(&mut self, text: &str) -> bool {
        if self.peek_text(text) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek_text(&self, text: &str) -> bool {
        self.tokens
            .get(self.position)
            .is_some_and(|token| token.text == text)
    }

    fn peek_text_value(&self) -> Option<&str> {
        self.tokens
            .get(self.position)
            .map(|token| token.text.as_str())
    }

    fn peek_kind(&self, kind: TokenKind) -> bool {
        self.tokens
            .get(self.position)
            .is_some_and(|token| token.kind == kind)
    }

    fn peek_word(&self) -> bool {
        self.tokens.get(self.position).is_some_and(is_symbol_name)
    }

    fn peek_unit_atom(&self) -> bool {
        self.peek_offset_unit_atom(0)
    }

    fn peek_offset_unit_atom(&self, offset: usize) -> bool {
        let Some(token) = self.tokens.get(self.position + offset) else {
            return false;
        };
        is_unit_token_atom(token)
    }
}

impl ExpressionUnit {
    fn from_unit(label: String, unit: String, range: TextRange) -> Self {
        Self {
            label,
            unit: UnitFactors::from_text(&unit),
            unit_text: Some(unit.clone()),
            poset: Some(PosetRef::Unit(parse_unit_expression_text(&unit))),
            scalar: true,
            range,
        }
    }

    fn from_symbol_text(label: String, range: TextRange, unit: &SymbolText) -> Self {
        Self::from_unit(label, unit.text.clone(), range)
    }

    fn from_port_symbol(label: String, range: TextRange, port: &PortSymbol) -> Self {
        let unit_text = port
            .poset
            .as_ref()
            .and_then(poset_unit_text)
            .or_else(|| port.unit.as_ref().map(|unit| unit.text.clone()));
        Self {
            label,
            unit: unit_text
                .as_deref()
                .map_or_else(UnitFactors::one, UnitFactors::from_text),
            unit_text,
            poset: port.poset.clone(),
            scalar: port
                .poset
                .as_ref()
                .is_none_or(|poset| !matches!(poset, PosetRef::Product(_))),
            range,
        }
    }

    fn field_poset(&self, field: &str) -> Option<PosetRef> {
        product_field_poset(self.poset.as_ref()?, field)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstanceScopedReference {
    pub(crate) port_name: String,
    pub(crate) port_range: TextRange,
    pub(crate) instance_name: String,
    pub(crate) instance_range: TextRange,
    pub(crate) direction: PortDirection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortDirection {
    Provided,
    Required,
}

impl PortDirection {
    fn hover_label(self) -> &'static str {
        match self {
            Self::Provided => "Provided",
            Self::Required => "Required",
        }
    }

    fn role_description(self) -> &'static str {
        match self {
            Self::Provided => "functionality/output that the model can provide",
            Self::Required => "resource/input that the model needs",
        }
    }

    fn diagnostic_label(self) -> &'static str {
        match self {
            Self::Provided => "provided",
            Self::Required => "required",
        }
    }

    fn declaration_keyword(self) -> &'static str {
        match self {
            Self::Provided => "`provides`",
            Self::Required => "`requires`",
        }
    }
}

fn project_sources(uri: &Url) -> HashMap<Url, String> {
    let Some(root) = uri_directory(uri) else {
        return HashMap::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return HashMap::new();
    };

    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_mcdpl_file(path))
        .filter_map(|path| {
            let uri = Url::from_file_path(&path).ok()?;
            let source = fs::read_to_string(path).ok()?;
            Some((uri, source))
        })
        .collect()
}

fn statement_tokens<'a>(tokens: &'a [Token], statement: &Statement) -> Vec<&'a Token> {
    tokens
        .iter()
        .filter(|token| {
            !is_trivia(token.kind)
                && token.range.start >= statement.range.start
                && token.range.end <= statement.range.end
        })
        .collect()
}

fn port_symbol(source: &str, statement: &Statement, tokens: &[&Token]) -> Option<PortSymbol> {
    let name_index = tokens
        .iter()
        .position(|token| matches!(token.text.as_str(), "provides" | "requires"))?
        + 1;
    let name = tokens.get(name_index)?;
    if !is_symbol_name(name) {
        return None;
    }

    Some(PortSymbol {
        name: name.text.clone(),
        name_range: name.range,
        unit: bracket_expression(source, tokens, name_index + 1),
        poset: None,
        declaration_range: statement.range,
    })
}

fn semantic_port_poset(
    model: Option<&SemanticModel>,
    direction: PortDirection,
    name: &str,
) -> Option<PosetRef> {
    let compiler_direction = match direction {
        PortDirection::Provided => LanguagePortDirection::Provides,
        PortDirection::Required => LanguagePortDirection::Requires,
    };
    model?
        .ports
        .iter()
        .find(|port| port.direction == compiler_direction && port.name == name)
        .map(|port| port.poset.clone())
}

fn assignment_symbol(
    source: &str,
    statement: &Statement,
    tokens: &[&Token],
) -> Option<AssignmentSymbol> {
    let equals_index = tokens.iter().position(|token| token.text.as_str() == "=")?;
    let name = tokens.first()?;
    if !is_symbol_name(name) || equals_index == 0 {
        return None;
    }
    let expression_start = tokens
        .get(equals_index + 1)
        .map_or(statement.range.end, |token| token.range.start);
    let expression_range = trim_range(
        source,
        TextRange::new(expression_start, statement.range.end),
    );

    Some(AssignmentSymbol {
        name: name.text.clone(),
        name_range: name.range,
        unit: bracket_expression(source, tokens, equals_index + 1),
        expression_range,
        declaration_range: statement.range,
    })
}

fn instance_binding(statement: &Statement, tokens: &[&Token]) -> Option<InstanceBinding> {
    let instance_index = tokens
        .iter()
        .position(|token| token.text.as_str() == "instance")?;
    let name_index = if tokens.first().map(|token| token.text.as_str()) == Some("sub") {
        1
    } else {
        0
    };
    let name = tokens.get(name_index)?;
    if !is_symbol_name(name) || name_index >= instance_index {
        return None;
    }

    Some(InstanceBinding {
        name: name.text.clone(),
        name_range: name.range,
        model: model_reference_after(tokens, instance_index + 1),
        declaration_range: statement.range,
    })
}

fn model_references(tokens: &[Token]) -> Vec<ModelReference> {
    let significant: Vec<&Token> = tokens
        .iter()
        .filter(|token| !is_trivia(token.kind))
        .collect();
    let mut references = Vec::new();

    for (index, token) in significant.iter().enumerate() {
        if token.text.as_str() != "`" {
            continue;
        }
        if let Some(reference) = model_reference_at(&significant, index) {
            references.push(reference);
        }
    }

    references
}

fn model_reference_after(tokens: &[&Token], start: usize) -> Option<ModelReference> {
    (start..tokens.len())
        .find(|index| tokens[*index].text.as_str() == "`")
        .and_then(|index| model_reference_at(tokens, index))
}

fn model_reference_at(tokens: &[&Token], backtick_index: usize) -> Option<ModelReference> {
    let backtick = tokens.get(backtick_index)?;
    let name = tokens.get(backtick_index + 1)?;
    if !is_symbol_name(name) {
        return None;
    }

    Some(ModelReference {
        name: name.text.clone(),
        name_range: name.range,
        reference_range: TextRange::new(backtick.range.start, name.range.end),
    })
}

fn resource_binding(statement: &Statement, tokens: &[&Token]) -> Option<ResourceBinding> {
    let resource_index = tokens
        .iter()
        .position(|token| token.text.as_str() == "resource")?;
    let path = tokens
        .iter()
        .skip(resource_index + 1)
        .find(|token| token.kind == TokenKind::String)?;
    let path_range = quoted_content_range(path);

    Some(ResourceBinding {
        path: strip_quotes(&path.text).to_owned(),
        path_range,
        expression_range: TextRange::new(tokens[resource_index].range.start, path.range.end),
        declaration_range: statement.range,
    })
}

fn relation_constraint(
    source: &str,
    statement: &Statement,
    tokens: &[&Token],
) -> Option<RelationConstraint> {
    let relation = tokens
        .iter()
        .find(|token| is_relation_operator(&token.text))?;
    let left_range = trim_range(
        source,
        TextRange::new(statement.range.start, relation.range.start),
    );
    let right_range = trim_range(
        source,
        TextRange::new(relation.range.end, statement.range.end),
    );
    if left_range.is_empty() || right_range.is_empty() {
        return None;
    }

    Some(RelationConstraint {
        left_range,
        relation_range: relation.range,
        right_range,
        statement_range: statement.range,
    })
}

fn bracket_expression(source: &str, tokens: &[&Token], start: usize) -> Option<SymbolText> {
    let open_index = (start..tokens.len()).find(|index| tokens[*index].text.as_str() == "[")?;
    let close_index = matching_close(tokens, open_index, "[", "]")?;
    let raw_range = TextRange::new(
        tokens[open_index].range.end,
        tokens[close_index].range.start,
    );
    let range = trim_range(source, raw_range);
    if range.is_empty() {
        return None;
    }

    Some(SymbolText {
        text: source[range.start..range.end].to_owned(),
        range,
    })
}

fn matching_close(
    tokens: &[&Token],
    open_index: usize,
    open_text: &str,
    close_text: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.text.as_str() == open_text {
            depth += 1;
            continue;
        }
        if token.text.as_str() == close_text {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn matching_close_tokens(
    tokens: &[Token],
    open_index: usize,
    open_text: &str,
    close_text: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.text.as_str() == open_text {
            depth += 1;
            continue;
        }
        if token.text.as_str() == close_text {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn trim_range(source: &str, range: TextRange) -> TextRange {
    let text = &source[range.start..range.end];
    let leading = text
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(text.len(), |(index, _)| index);
    let trailing = text
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(leading, |(index, ch)| index + ch.len_utf8());

    TextRange::new(range.start + leading, range.start + trailing)
}

fn quoted_content_range(token: &Token) -> TextRange {
    let mut chars = token.text.char_indices();
    let Some((_, quote)) = chars.next() else {
        return token.range;
    };
    if !matches!(quote, '"' | '\'') || !token.text.ends_with(quote) || token.text.len() < 2 {
        return token.range;
    }

    TextRange::new(
        token.range.start + quote.len_utf8(),
        token.range.end - quote.len_utf8(),
    )
}

fn strip_quotes(text: &str) -> &str {
    let Some(first) = text.chars().next() else {
        return text;
    };
    if !matches!(first, '"' | '\'') || !text.ends_with(first) || text.len() < 2 {
        return text;
    }

    &text[first.len_utf8()..text.len() - first.len_utf8()]
}

fn is_symbol_name(token: &Token) -> bool {
    matches!(token.kind, TokenKind::Ident | TokenKind::Keyword)
}

fn is_unit_token_atom(token: &Token) -> bool {
    if matches!(
        token.text.as_str(),
        "provided" | "required" | "by" | "sum" | "constant" | "true" | "false"
    ) {
        return false;
    }
    is_symbol_name(token)
        || token.kind == TokenKind::Number
        || matches!(token.text.as_str(), "$" | "°")
}

fn is_relation_operator(text: &str) -> bool {
    matches!(text, "<=" | ">=" | "≤" | "≥" | "=" | "==")
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment
    )
}

fn uri_directory(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn is_mcdpl_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("mcdp" | "mcdp_interface" | "mcdp_template" | "mcdp_poset")
    )
}

fn model_name(uri: &Url) -> Option<String> {
    uri.to_file_path().ok().and_then(|path| {
        path.file_stem()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

fn document_extension(uri: &Url) -> Option<String> {
    uri.to_file_path().ok().and_then(|path| {
        path.extension()
            .map(|extension| extension.to_string_lossy().into_owned())
    })
}

fn document_priority(uri: &Url) -> usize {
    let Ok(path) = uri.to_file_path() else {
        return usize::MAX;
    };
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("mcdp") => 0,
        Some("mcdp_interface") => 1,
        Some("mcdp_template") => 2,
        Some("mcdp_poset") => 3,
        _ => usize::MAX,
    }
}

fn contains_offset(range: TextRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

fn merge_range(left: TextRange, right: TextRange) -> TextRange {
    TextRange::new(left.start.min(right.start), left.end.max(right.end))
}

fn unit_base_and_exponent(token: &str) -> (String, i32) {
    if let Some((base, exponent)) = token.split_once('^')
        && let Ok(exponent) = exponent.parse::<i32>()
    {
        return (base.to_owned(), exponent);
    }
    if let Some((base, exponent)) = split_superscript_unit(token) {
        return (base, exponent);
    }
    (token.to_owned(), 1)
}

fn dimension_unit_atom_factors(atom: &str) -> Vec<(String, i32)> {
    match atom.trim() {
        "$" => vec![("USD".to_owned(), 1)],
        "N" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 1),
            ("s".to_owned(), -2),
        ],
        "Nm" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -2),
        ],
        "W" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -3),
        ],
        "J" | "kJ" | "Wh" | "kWh" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -2),
        ],
        "g" | "mg" => vec![("kg".to_owned(), 1)],
        "deg" | "degree" | "degrees" | "°" => vec![("rad".to_owned(), 1)],
        "h" | "hr" | "hour" | "hours" | "min" | "minute" | "minutes" => {
            vec![("s".to_owned(), 1)]
        }
        other => vec![(other.to_owned(), 1)],
    }
}

fn split_superscript_unit(token: &str) -> Option<(String, i32)> {
    let mut exponent_digits = String::new();
    let mut split_index = token.len();
    for (index, ch) in token.char_indices().rev() {
        let Some(digit) = superscript_digit(ch) else {
            break;
        };
        exponent_digits.insert(0, digit);
        split_index = index;
    }
    if exponent_digits.is_empty() || split_index == 0 {
        return None;
    }
    let exponent = exponent_digits.parse::<i32>().ok()?;
    Some((token[..split_index].to_owned(), exponent))
}

fn superscript_digit(ch: char) -> Option<char> {
    match ch {
        '⁰' => Some('0'),
        '¹' => Some('1'),
        '²' => Some('2'),
        '³' => Some('3'),
        '⁴' => Some('4'),
        '⁵' => Some('5'),
        '⁶' => Some('6'),
        '⁷' => Some('7'),
        '⁸' => Some('8'),
        '⁹' => Some('9'),
        _ => None,
    }
}

fn unit_factor_label(unit: &str, exponent: i32) -> String {
    if exponent == 1 {
        unit.to_owned()
    } else {
        format!("{unit}^{exponent}")
    }
}

fn poset_unit_text(poset: &PosetRef) -> Option<String> {
    match poset {
        PosetRef::Unit(unit) => Some(unit_expression_text(unit)),
        PosetRef::Builtin(name) => Some(name.clone()),
        PosetRef::Named(name) => Some(format!("`{name}")),
        PosetRef::Raw(raw) => Some(raw.clone()),
        PosetRef::Product(_) => None,
    }
}

fn product_field_poset(poset: &PosetRef, field: &str) -> Option<PosetRef> {
    match poset {
        PosetRef::Product(fields) => fields
            .iter()
            .find(|candidate| candidate.name == field)
            .map(|candidate| candidate.poset.clone()),
        _ => None,
    }
}

fn unit_expression_text(unit: &UnitExpression) -> String {
    match unit {
        UnitExpression::One => "dimensionless".to_owned(),
        UnitExpression::Symbol(symbol) => symbol.clone(),
        UnitExpression::Product(parts) => parts
            .iter()
            .map(unit_expression_text)
            .collect::<Vec<_>>()
            .join("*"),
        UnitExpression::Quotient {
            numerator,
            denominator,
        } => {
            format!(
                "{}/{}",
                unit_expression_text(numerator),
                unit_expression_text(denominator)
            )
        }
        UnitExpression::Power { base, exponent } => {
            format!("{}^{exponent}", unit_expression_text(base))
        }
        UnitExpression::Raw(raw) => raw.clone(),
    }
}

fn unit_atoms(text: &str) -> Vec<UnitAtom> {
    let trimmed = text.trim();
    if let Some(inner) = constructor_inner(trimmed, "product") {
        let mut atoms = Vec::new();
        for field in split_top_level(inner, ',') {
            if let Some((_, poset)) = split_top_level_once(&field, ':') {
                atoms.extend(unit_atoms(poset.trim()));
            }
        }
        return atoms;
    }

    let mut atoms = Vec::new();
    let mut chars = trimmed.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch == '`' {
            let atom = consume_unit_atom(&mut chars);
            if !atom.is_empty() {
                atoms.push(UnitAtom {
                    name: atom,
                    named: true,
                });
            }
            continue;
        }
        if is_unit_atom_start(ch) {
            let mut atom = String::new();
            atom.push(ch);
            while let Some((_, next)) = chars.peek().copied() {
                if !is_unit_atom_continue(next) {
                    break;
                }
                atom.push(next);
                chars.next();
            }
            if !is_unit_constructor(&atom) {
                atoms.push(UnitAtom {
                    name: atom,
                    named: false,
                });
            }
            continue;
        }
        if ch == '$' || ch == '°' {
            atoms.push(UnitAtom {
                name: trimmed[start..start + ch.len_utf8()].to_owned(),
                named: false,
            });
        }
    }

    atoms
}

fn consume_unit_atom(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> String {
    let mut atom = String::new();
    while let Some((_, ch)) = chars.peek().copied() {
        if !is_unit_atom_continue(ch) {
            break;
        }
        atom.push(ch);
        chars.next();
    }
    atom
}

fn is_unit_atom_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_unit_atom_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_unit_constructor(atom: &str) -> bool {
    matches!(
        atom,
        "product"
            | "LowerSet"
            | "LowerSets"
            | "lower_set"
            | "lower_sets"
            | "UpperSet"
            | "UpperSets"
            | "upper_set"
            | "upper_sets"
    )
}

fn is_base_unit_atom(atom: &str) -> bool {
    primitive_posets().iter().any(|poset| poset.name == atom)
        || matches!(
            atom,
            "1" | "dimensionless"
                | "unitless"
                | "$"
                | "mg"
                | "deg"
                | "degree"
                | "degrees"
                | "°"
                | "kJ"
                | "Wh"
                | "kWh"
                | "h"
                | "hr"
                | "hour"
                | "hours"
                | "min"
                | "minute"
                | "minutes"
        )
}

fn constructor_inner<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
}

fn split_top_level(text: &str, delimiter: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                parts.push(text[start..index].trim().to_owned());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim().to_owned());
    parts
}

fn split_top_level_once(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                let right = index + ch.len_utf8();
                return Some((&text[..index], &text[right..]));
            }
            _ => {}
        }
    }
    None
}

fn term_hover_text(text: &str) -> Option<&'static str> {
    match text {
        "dp" => Some(
            "**Document kind** `dp`\n\nA `dp` document defines one design problem: the functionality it provides and the resources it requires.",
        ),
        "mcdp" => Some(
            "**Document kind** `mcdp`\n\nAn `mcdp` document composes design-problem instances, connects their ports, and exposes a public co-design interface.",
        ),
        "provides" => Some(
            "**Keyword** `provides`\n\nDeclares a functionality/output produced by the current model.",
        ),
        "requires" => {
            Some("**Keyword** `requires`\n\nDeclares a resource/input needed by the current model.")
        }
        "provided" => Some(
            "**Keyword** `provided`\n\nRefers to the functionality/output side of a variable or instance-scoped expression.",
        ),
        "required" => Some(
            "**Keyword** `required`\n\nRefers to the requirement/input side of a variable or instance-scoped expression.",
        ),
        "instance" => Some(
            "**Keyword** `instance`\n\nCreates a local binding to another MCDPL model. Instance-scoped variables resolve through this binding.",
        ),
        "implemented-by" => Some(
            "**Keyword** `implemented-by`\n\nConnects a design problem to an external implementation resource, usually a YAML catalog.",
        ),
        "resource" => Some(
            "**Function** `resource(...)`\n\nPoints to an external file used by an implementation binding.",
        ),
        "interface" => Some(
            "**Document kind** `interface`\n\nDeclares a reusable set of provided and required ports without an implementation.",
        ),
        "poset" => Some(
            "**Document kind** `poset`\n\nDeclares an ordered value space used by variables and units.",
        ),
        "catalog" => Some(
            "**Document kind** `catalog`\n\nDeclares implementation records for a design problem.",
        ),
        "template" => Some(
            "**Document kind** `template`\n\nDefines a parameterized MCDPL model that can be specialized later.",
        ),
        _ => None,
    }
}

fn primitive_poset_hover_text(text: &str) -> Option<String> {
    let primitive = primitive_posets()
        .iter()
        .find(|primitive| primitive.name == text)?;
    Some(format!(
        "**Primitive poset** `{}`\n\n{}.",
        primitive.name, primitive.description
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn indexes_port_instance_model_resource_and_unit_symbols() {
        let uri = test_file_url(Path::new("/tmp/fleet.mcdp"));
        let source = "\
mcdp {
  provides number_t1 [car]
  requires total_cost [USD]
  sub dp_t1 = instance `fleet_type_1
  battery_dp = instance `battery
  implemented-by yaml resource(\"catalogs/fleet.yaml\")
}
";

        let symbols = DocumentSymbols::parse(uri, source);

        assert_eq!(symbols.kind, Some(DocumentKind::Mcdp));
        assert_eq!(names(&symbols.provides), vec!["number_t1"]);
        assert_eq!(symbols.provides[0].unit_text(), Some("car"));
        assert_eq!(names(&symbols.requires), vec!["total_cost"]);
        assert_eq!(symbols.requires[0].unit_text(), Some("USD"));
        assert_eq!(
            instance_names(&symbols.instances),
            vec!["dp_t1", "battery_dp"]
        );
        assert_eq!(
            symbols.instances[0]
                .model
                .as_ref()
                .map(|model| model.name.as_str()),
            Some("fleet_type_1")
        );
        assert_eq!(
            symbols.instances[1]
                .model
                .as_ref()
                .map(|model| model.name.as_str()),
            Some("battery")
        );
        assert_eq!(
            model_reference_names(&symbols),
            vec!["fleet_type_1", "battery"]
        );
        assert_eq!(resource_paths(&symbols), vec!["catalogs/fleet.yaml"]);
        assert_eq!(unit_names(&symbols), vec!["car", "USD"]);
    }

    #[test]
    fn indexes_multiline_product_unit_expression() {
        let uri = test_file_url(Path::new("/tmp/planner.mcdp"));
        let source = "\
dp {
  requires dyn_prop [product(v: m/s,
                             max_lateral_a: m/s²,
                             path_type: `path_type)]
}
";

        let symbols = DocumentSymbols::parse(uri, source);

        assert_eq!(names(&symbols.requires), vec!["dyn_prop"]);
        assert_eq!(
            symbols.requires[0].unit_text(),
            Some(
                "product(v: m/s,\n                             max_lateral_a: m/s²,\n                             path_type: `path_type)"
            )
        );
        assert_eq!(model_reference_names(&symbols), vec!["path_type"]);
    }

    #[test]
    fn project_index_scans_sibling_mcdpl_files_and_overlays_open_documents() {
        let temp_dir = TempDir::new("project-symbol-index");
        temp_dir.write("fleet.mcdp", "mcdp { provides old_name [Nat] }");
        temp_dir.write("fleet_type_1.mcdp", "dp { provides operators [Nat] }");
        temp_dir.write("fleet.yaml", "[]");

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let mut open_documents = HashMap::new();
        open_documents.insert(
            fleet_uri.clone(),
            "mcdp { provides number_t1 [car]\n sub dp_t1 = instance `fleet_type_1 }".to_owned(),
        );

        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &open_documents);

        assert_eq!(index.documents.len(), 2);
        let fleet_symbols = document(&index, &fleet_uri);
        assert_eq!(names(&fleet_symbols.provides), vec!["number_t1"]);
        assert_eq!(instance_names(&fleet_symbols.instances), vec!["dp_t1"]);
        assert!(
            document(&index, &temp_dir.url("fleet_type_1.mcdp"))
                .provides
                .iter()
                .any(|port| port.name == "operators")
        );
    }

    #[test]
    fn resolves_model_reference_definition_to_matching_model_file() {
        let temp_dir = TempDir::new("model-reference-definition");
        let fleet_source = "\
mcdp {
  sub dp_t1 = instance `fleet_type_1
}
";
        let target_source = "dp { provides operators [Nat] }";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fleet_type_1.mcdp", target_source);

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let target_uri = temp_dir.url("fleet_type_1.mcdp");
        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &HashMap::new());

        let target = match index.definition_at(&fleet_uri, offset_of(fleet_source, "fleet_type_1"))
        {
            Some(target) => target,
            None => panic!("missing definition target for model reference"),
        };

        assert_eq!(target.uri, target_uri);
        assert_eq!(text_at(target_source, target.range), "dp");
    }

    #[test]
    fn resolves_instance_scoped_variable_to_instance_model_port() {
        let temp_dir = TempDir::new("instance-scoped-definition");
        let fleet_source = "\
mcdp {
  sub dp_t1 = instance `fleet_type_1
  required total_num_operators >= operators required by dp_t1
}
";
        let target_source = "dp { requires operators [Nat] }";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fleet_type_1.mcdp", target_source);

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let target_uri = temp_dir.url("fleet_type_1.mcdp");
        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &HashMap::new());

        let target = match index.definition_at(
            &fleet_uri,
            offset_of(fleet_source, "operators required by dp_t1"),
        ) {
            Some(target) => target,
            None => panic!("missing definition target for instance-scoped variable"),
        };

        assert_eq!(target.uri, target_uri);
        assert_eq!(text_at(target_source, target.range), "operators");
    }

    #[test]
    fn references_for_model_reference_use_resolved_model_identity() {
        let temp_dir = TempDir::new("model-reference-occurrences");
        let fleet_source = "\
mcdp {
  sub dp_t1 = instance `fleet_type_1
  sub dp_t1_backup = instance `fleet_type_1
  sub dp_t2 = instance `fleet_type_2
}
";
        let target_source = "dp { requires operators [Nat] }";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fleet_type_1.mcdp", target_source);
        temp_dir.write("fleet_type_2.mcdp", "dp { requires operators [Nat] }");

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let target_uri = temp_dir.url("fleet_type_1.mcdp");
        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &HashMap::new());

        let references = must_option(
            index.references_at(&fleet_uri, offset_of(fleet_source, "fleet_type_1"), false),
            "missing model references",
        );
        assert_eq!(
            occurrence_texts(&index, &references),
            vec!["fleet_type_1", "fleet_type_1",]
        );

        let references_with_declaration = must_option(
            index.references_at(&fleet_uri, offset_of(fleet_source, "fleet_type_1"), true),
            "missing model references with declaration",
        );
        assert!(
            references_with_declaration
                .iter()
                .any(|occurrence| occurrence.uri == target_uri
                    && text_at(target_source, occurrence.range) == "dp")
        );
    }

    #[test]
    fn references_for_instance_scoped_variable_use_resolved_port_identity() {
        let temp_dir = TempDir::new("instance-scoped-occurrences");
        let fleet_source = "\
mcdp {
  sub dp_t1 = instance `fleet_type_1
  sub dp_t2 = instance `fleet_type_2
  required total_num_operators >= operators required by dp_t1
  required total_num_drivers >= operators required by dp_t2
}
";
        let target_source = "dp { requires operators [Nat] }";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fleet_type_1.mcdp", target_source);
        temp_dir.write("fleet_type_2.mcdp", "dp { requires operators [Nat] }");

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let target_uri = temp_dir.url("fleet_type_1.mcdp");
        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &HashMap::new());

        let references = must_option(
            index.references_at(
                &fleet_uri,
                offset_of(fleet_source, "operators required by dp_t1"),
                false,
            ),
            "missing instance-scoped references",
        );
        assert_eq!(occurrence_texts(&index, &references), vec!["operators"]);
        assert_eq!(references[0].uri, fleet_uri);

        let highlights = must_option(
            index.document_occurrences_at(
                &fleet_uri,
                offset_of(fleet_source, "operators required by dp_t1"),
            ),
            "missing instance-scoped document highlights",
        );
        assert_eq!(occurrence_texts(&index, &highlights), vec!["operators"]);

        let references_with_declaration = must_option(
            index.references_at(
                &fleet_uri,
                offset_of(fleet_source, "operators required by dp_t1"),
                true,
            ),
            "missing instance-scoped references with declaration",
        );
        assert!(
            references_with_declaration
                .iter()
                .any(|occurrence| occurrence.uri == target_uri
                    && text_at(target_source, occurrence.range) == "operators")
        );
    }

    #[test]
    fn hover_explains_document_kind_and_resolved_symbols() {
        let temp_dir = TempDir::new("symbol-hover");
        let fleet_source = "\
mcdp {
  sub dp_t1 = instance `fleet_type_1
  required total_num_operators >= operators required by dp_t1
}
";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fleet_type_1.mcdp", "dp { requires operators [Nat] }");

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let target_uri = temp_dir.url("fleet_type_1.mcdp");
        let index = ProjectSymbolIndex::for_uri(&fleet_uri, &HashMap::new());

        let document_kind_hover = must_option(
            index.hover_at(&target_uri, 0),
            "missing document kind hover",
        );
        assert!(document_kind_hover.contents.contains("`dp` document"));

        let model_hover = must_option(
            index.hover_at(&fleet_uri, offset_of(fleet_source, "fleet_type_1")),
            "missing model reference hover",
        );
        assert!(model_hover.contents.contains("Model reference"));
        assert!(model_hover.contents.contains("fleet_type_1"));

        let variable_hover = must_option(
            index.hover_at(
                &fleet_uri,
                offset_of(fleet_source, "operators required by dp_t1"),
            ),
            "missing instance-scoped variable hover",
        );
        assert!(variable_hover.contents.contains("Required variable"));
        assert!(variable_hover.contents.contains("dp_t1"));
    }

    #[test]
    fn hover_explains_primitive_posets_and_units() {
        let temp_dir = TempDir::new("primitive-poset-hover");
        let source = "\
dp {
  provides count [Nat]
  requires energy [J]
}
";
        temp_dir.write("unit.mcdp", source);
        let uri = temp_dir.url("unit.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let nat_hover = must_option(
            index.hover_at(&uri, offset_of(source, "Nat")),
            "missing Nat hover",
        );
        assert!(nat_hover.contents.contains("Natural numbers"));

        let joule_hover = must_option(
            index.hover_at(&uri, offset_of(source, "J")),
            "missing J hover",
        );
        assert!(joule_hover.contents.contains("joule"));
    }

    #[test]
    fn reports_undefined_custom_units_and_named_posets() {
        let temp_dir = TempDir::new("undefined-units");
        let source = "\
mcdp {
  provides vehicles [car]
  requires mode [`missing_mode]
  requires totals [product(cost: USD, mass: kg)]
}
";
        temp_dir.write("fleet.mcdp", source);
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(has_diagnostic(&diagnostics, "lsp.undefined-unit", "car"));
        assert!(has_diagnostic(
            &diagnostics,
            "lsp.undefined-poset",
            "missing_mode"
        ));
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("USD"))
        );
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("kg"))
        );
    }

    #[test]
    fn reports_undefined_instance_references() {
        let temp_dir = TempDir::new("undefined-instance");
        let source = "\
mcdp {
  requires total_cost [USD]
  # sub fu = instance `fuel

  required total_cost >= fuel_cost required by fu
}
";
        temp_dir.write("fleet.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires fuel_cost [USD] }");
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(has_diagnostic(&diagnostics, "lsp.undefined-instance", "fu"));
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "lsp.undefined-instance")
            .expect("undefined instance diagnostic should exist");
        assert_eq!(text_at(source, diagnostic.range), "fu");
    }

    #[test]
    fn reports_missing_ports_on_resolved_instances() {
        let temp_dir = TempDir::new("undefined-instance-port");
        let source = "\
mcdp {
  requires total_cost [USD]
  sub fu = instance `fuel

  required total_cost >= fuel_cost required by fu
}
";
        temp_dir.write("fleet.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires emissions [kg] }");
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(has_diagnostic(
            &diagnostics,
            "lsp.undefined-required-port",
            "fuel_cost"
        ));
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "lsp.undefined-required-port")
            .expect("undefined required port diagnostic should exist");
        assert_eq!(text_at(source, diagnostic.range), "fuel_cost");
    }

    #[test]
    fn accepts_project_defined_custom_units_and_named_posets() {
        let temp_dir = TempDir::new("defined-units");
        let source = "\
mcdp {
  provides vehicles [car]
  requires mode [`mode]
}
";
        temp_dir.write("fleet.mcdp", source);
        temp_dir.write("car.mcdp_poset", "poset { compact full_size }");
        temp_dir.write("mode.mcdp_poset", "poset { regular express }");
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn reports_unit_mismatches_across_resolved_instance_ports() {
        let temp_dir = TempDir::new("unit-mismatch");
        let source = "\
mcdp {
  requires total_cost [USD]
  sub fu = instance `fuel

  required total_cost >= emissions required by fu
}
";
        temp_dir.write("fleet.mcdp", source);
        temp_dir.write("fuel.mcdp", "dp { requires emissions [kg] }");
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(has_diagnostic(
            &diagnostics,
            "lsp.unit-mismatch",
            "emissions"
        ));
        let mismatch = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "lsp.unit-mismatch")
            .expect("unit mismatch diagnostic should exist");
        assert_eq!(text_at(source, mismatch.range), "emissions");
    }

    #[test]
    fn does_not_flag_ports_inside_unit_converting_product_terms() {
        let temp_dir = TempDir::new("unit-product-conversion");
        let source = "\
mcdp {
  provides total_load [kg]
  provides velocity [m/s]
  requires power [W]
  requires total_mass [kg]

  self_mass = 770 [kg]
  gain_mass_velocity_to_power = 1 [W*s/m/kg]
  min_required_power = 10 [W]

  required power >= min_required_power + gain_mass_velocity_to_power * (provided total_load + self_mass) * provided velocity
  required total_mass >= provided total_load + self_mass
}
";
        temp_dir.write("mechanics.mcdp", source);
        let uri = temp_dir.url("mechanics.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "lsp.unit-mismatch"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn reports_unit_mismatches_after_product_unit_algebra() {
        let temp_dir = TempDir::new("unit-product-mismatch");
        let source = "\
mcdp {
  provides total_load [kg]
  provides velocity [m/s]
  requires power [W]

  gain_mass_velocity_to_power = 1 [s/m/kg]

  required power >= gain_mass_velocity_to_power * provided total_load * provided velocity
}
";
        temp_dir.write("mechanics.mcdp", source);
        let uri = temp_dir.url("mechanics.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        let mismatch = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "lsp.unit-mismatch")
            .expect("unit mismatch diagnostic should exist");
        assert!(
            text_at(source, mismatch.range).contains("gain_mass_velocity_to_power"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn resolves_product_field_units_from_compiler_posets() {
        let temp_dir = TempDir::new("product-field-units");
        let source = "\
mcdp {
  requires budget [USD]
  requires mass [kg]
  requires cost_and_mass [product(overall_cost: USD, total_mass: kg)]

  required budget >= (required cost_and_mass).overall_cost
  required budget >= (required cost_and_mass).total_mass
}
";
        temp_dir.write("rover.mcdp", source);
        let uri = temp_dir.url("rover.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "lsp.unit-mismatch")
                .count(),
            1,
            "{diagnostics:?}"
        );
        let mismatch = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "lsp.unit-mismatch")
            .expect("unit mismatch diagnostic should exist");
        assert_eq!(text_at(source, mismatch.range), "total_mass");
    }

    #[test]
    fn diagnostic_refresh_uris_include_dependent_open_documents() {
        let temp_dir = TempDir::new("diagnostic-refresh-uris");
        let fleet_source = "\
mcdp {
  sub fu = instance `fuel
  required total_cost >= fuel_cost required by fu
}
";
        let fuel_source = "dp { requires fuel_cost [USD] }";
        let unrelated_source = "dp { provides speed [m/s] }";
        temp_dir.write("fleet.mcdp", fleet_source);
        temp_dir.write("fuel.mcdp", fuel_source);
        temp_dir.write("unrelated.mcdp", unrelated_source);

        let fleet_uri = temp_dir.url("fleet.mcdp");
        let fuel_uri = temp_dir.url("fuel.mcdp");
        let unrelated_uri = temp_dir.url("unrelated.mcdp");
        let open_uris = [fleet_uri.clone(), fuel_uri.clone(), unrelated_uri.clone()];
        let index = ProjectSymbolIndex::for_uri(&fuel_uri, &HashMap::new());

        let refresh_uris = index.diagnostic_refresh_uris(&fuel_uri, open_uris.iter());

        assert!(refresh_uris.contains(&fleet_uri));
        assert!(refresh_uris.contains(&fuel_uri));
        assert!(!refresh_uris.contains(&unrelated_uri));
    }

    #[test]
    fn accepts_dimensionally_equivalent_units_in_relations() {
        let temp_dir = TempDir::new("unit-compatible");
        let source = "\
mcdp {
  requires mass [kg]
  sub supplier = instance `supplier

  required mass >= supplied_mass provided by supplier
}
";
        temp_dir.write("fleet.mcdp", source);
        temp_dir.write("supplier.mcdp", "dp { provides supplied_mass [g] }");
        let uri = temp_dir.url("fleet.mcdp");
        let index = ProjectSymbolIndex::for_uri(&uri, &HashMap::new());

        let diagnostics = index.semantic_diagnostics(&uri);

        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "lsp.unit-mismatch"),
            "{diagnostics:?}"
        );
    }

    impl PortSymbol {
        fn unit_text(&self) -> Option<&str> {
            self.unit.as_ref().map(|unit| unit.text.as_str())
        }
    }

    fn names(ports: &[PortSymbol]) -> Vec<&str> {
        ports.iter().map(|port| port.name.as_str()).collect()
    }

    fn instance_names(instances: &[InstanceBinding]) -> Vec<&str> {
        instances
            .iter()
            .map(|instance| instance.name.as_str())
            .collect()
    }

    fn model_reference_names(symbols: &DocumentSymbols) -> Vec<&str> {
        symbols
            .model_references
            .iter()
            .map(|reference| reference.name.as_str())
            .collect()
    }

    fn resource_paths(symbols: &DocumentSymbols) -> Vec<&str> {
        symbols
            .resource_bindings
            .iter()
            .map(|resource| resource.path.as_str())
            .collect()
    }

    fn unit_names(symbols: &DocumentSymbols) -> Vec<&str> {
        symbols
            .declared_units
            .iter()
            .map(|unit| unit.name.as_str())
            .collect()
    }

    fn has_diagnostic(diagnostics: &[ProjectDiagnostic], code: &str, text: &str) -> bool {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.message.contains(text))
    }

    fn document<'a>(index: &'a ProjectSymbolIndex, uri: &Url) -> &'a DocumentSymbols {
        match index.documents.get(uri) {
            Some(document) => document,
            None => panic!("missing document symbols for {uri}"),
        }
    }

    fn occurrence_texts(
        index: &ProjectSymbolIndex,
        occurrences: &[SymbolOccurrence],
    ) -> Vec<String> {
        occurrences
            .iter()
            .map(|occurrence| {
                let document = document(index, &occurrence.uri);
                text_at(&document.source, occurrence.range).to_owned()
            })
            .collect()
    }

    fn offset_of(source: &str, needle: &str) -> usize {
        match source.find(needle) {
            Some(offset) => offset,
            None => panic!("missing `{needle}` in test source"),
        }
    }

    fn text_at(source: &str, range: TextRange) -> &str {
        &source[range.start..range.end]
    }

    fn test_file_url(path: &Path) -> Url {
        match Url::from_file_path(path) {
            Ok(url) => url,
            Err(()) => panic!("could not convert test path to file URL"),
        }
    }

    fn must<T, E: std::fmt::Debug>(result: std::result::Result<T, E>, context: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    fn must_option<T>(option: Option<T>, context: &str) -> T {
        match option {
            Some(value) => value,
            None => panic!("{context}"),
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(duration) => duration.as_nanos(),
                Err(error) => panic!("could not read system time: {error:?}"),
            };
            let path =
                std::env::temp_dir().join(format!("mcdp-lsp-{name}-{}-{now}", std::process::id()));
            must(fs::create_dir_all(&path), "could not create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, text: &str) {
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                must(
                    fs::create_dir_all(parent),
                    "could not create temp file parent",
                );
            }
            must(fs::write(path, text), "could not write temp file");
        }

        fn url(&self, relative_path: &str) -> Url {
            test_file_url(&self.path.join(relative_path))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
