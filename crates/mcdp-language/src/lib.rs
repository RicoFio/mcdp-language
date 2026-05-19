//! Shared MCDPL language front-end APIs for editor and compiler clients.
//!
//! This crate owns the language-facing type universe: source spans,
//! diagnostics, syntax, unit expressions, lightweight poset references, and
//! statement-level declaration lowering. Solver and graph crates should adapt
//! these types instead of redefining editor/compiler frontend structures.

mod diagnostic;
mod expression;
mod graph;
mod poset;
mod semantic;
mod source;
mod syntax;
mod units;

pub use diagnostic::{CheckReport, Diagnostic, Severity};
pub use expression::{
    AggregateExpression, AggregateOperator, BinaryOperator, Expression, LiteralExpression,
    PortReference, QuantityLiteral, UnaryOperator, UnitExpression, parse_expression_list_text,
    parse_expression_text, parse_unit_expression_text,
};
pub use graph::{Constraint, DesignGraph, Node, Port, PortDirection, Relation};
pub use poset::{NamedPoset, PosetRef};
pub use semantic::{
    AssignmentDecl, BareExpressionDecl, CatalogRecordDecl, ChoiceDecl, ConstraintDecl,
    ImplementationDecl, ImportDecl, InstanceDecl, InterfaceImplDecl, ParameterDecl, PortDecl,
    SemanticModel, graph_from_semantic, lower_document,
};
pub use source::{SourceId, TextRange, TextSpan};
pub use syntax::{
    DocumentKind, ParsedDocument, Statement, StatementKind, SyntaxBody, SyntaxDocument,
    SyntaxEntry, Token, TokenKind, lex, parse_document,
};
pub use units::{
    canonical_unit_label, canonical_unit_option, normalize_unit_text, units_equivalent,
};
