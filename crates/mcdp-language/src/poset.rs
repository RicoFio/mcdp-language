//! Lightweight poset references used by parser, compiler frontend, and editor tooling.

use crate::UnitExpression;

/// Reference to a poset used by ports, catalogs, and values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PosetRef {
    /// Parsed unit or quantity poset expression.
    Unit(UnitExpression),
    /// Built-in or unit poset expression, such as `J`, `USD`, or `Nat`.
    Builtin(String),
    /// Named project poset loaded from a `.mcdp_poset` file.
    Named(String),
    /// Anonymous product.
    Product(Vec<NamedPoset>),
    /// Raw expression kept until lowering is complete.
    Raw(String),
}

/// A named field in a product poset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedPoset {
    /// Field name.
    pub name: String,
    /// Field poset.
    pub poset: PosetRef,
}

impl NamedPoset {
    /// Creates a named product field.
    #[must_use]
    pub fn new(name: impl Into<String>, poset: PosetRef) -> Self {
        Self {
            name: name.into(),
            poset,
        }
    }
}
