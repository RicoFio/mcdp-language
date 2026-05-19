//! Canonical design graph metadata used before full DP/DPI lowering.

use crate::PosetRef;

/// Functionality or requirement direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortDirection {
    /// Functionality provided by the design problem.
    Provides,
    /// Requirement/resource needed by the design problem.
    Requires,
}

/// Public interface port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Port {
    /// Port name.
    pub name: String,
    /// Functionality/resource direction.
    pub direction: PortDirection,
    /// Declared poset.
    pub poset: PosetRef,
}

impl Port {
    /// Creates a port.
    #[must_use]
    pub fn new(name: impl Into<String>, direction: PortDirection, poset: PosetRef) -> Self {
        Self {
            name: name.into(),
            direction,
            poset,
        }
    }
}

/// Subproblem node in a composite graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    /// Local instance name.
    pub name: String,
    /// Referenced model/template/catalog.
    pub model: String,
}

impl Node {
    /// Creates a subproblem node.
    #[must_use]
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model: model.into(),
        }
    }
}

/// Constraint relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Relation {
    /// Less-than-or-equal in the relevant poset.
    Leq,
    /// Greater-than-or-equal in the relevant poset.
    Geq,
    /// Equality.
    Eq,
}

/// Source-preserving constraint shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Constraint {
    /// Left expression text.
    pub left: String,
    /// Relation.
    pub relation: Relation,
    /// Right expression text.
    pub right: String,
}

impl Constraint {
    /// Creates a constraint.
    #[must_use]
    pub fn new(left: impl Into<String>, relation: Relation, right: impl Into<String>) -> Self {
        Self {
            left: left.into(),
            relation,
            right: right.into(),
        }
    }
}

/// Canonical graph shell for a named design problem.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesignGraph {
    /// Optional model name.
    pub name: Option<String>,
    /// Public interface ports.
    pub ports: Vec<Port>,
    /// Subproblem instances.
    pub nodes: Vec<Node>,
    /// Interconnection and bound constraints.
    pub constraints: Vec<Constraint>,
}
