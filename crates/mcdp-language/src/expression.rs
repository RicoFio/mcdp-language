//! Small expression, value, and unit parser for lowering.

use crate::PortDirection;

/// Parsed unit expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnitExpression {
    /// Unitless value.
    One,
    /// Atomic unit symbol, such as `m`, `s`, `USD`, or `dimensionless`.
    Symbol(String),
    /// Multiplicative unit expression.
    Product(Vec<UnitExpression>),
    /// Quotient unit expression.
    Quotient {
        /// Numerator unit expression.
        numerator: Box<UnitExpression>,
        /// Denominator unit expression.
        denominator: Box<UnitExpression>,
    },
    /// Power unit expression.
    Power {
        /// Base unit expression.
        base: Box<UnitExpression>,
        /// Exponent text.
        exponent: String,
    },
    /// Raw fallback when the unit parser cannot safely structure the expression.
    Raw(String),
}

/// Numeric quantity literal with a parsed unit expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuantityLiteral {
    /// Magnitude text.
    pub magnitude: String,
    /// Optional unit expression.
    pub unit: Option<UnitExpression>,
}

impl QuantityLiteral {
    /// Creates a quantity literal.
    #[must_use]
    pub fn new(magnitude: impl Into<String>, unit: Option<UnitExpression>) -> Self {
        Self {
            magnitude: magnitude.into(),
            unit,
        }
    }
}

/// Literal expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiteralExpression {
    /// Unit value.
    Unit,
    /// Boolean literal.
    Bool(bool),
    /// Numeric quantity literal.
    Quantity(QuantityLiteral),
    /// Symbolic finite-poset value, optionally qualified by a poset name.
    Symbol {
        /// Optional poset/type name before `:`.
        poset: Option<String>,
        /// Symbol value.
        value: String,
    },
    /// Positional tuple value.
    Tuple(Vec<Expression>),
}

/// Parsed expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expression {
    /// Literal expression.
    Literal(LiteralExpression),
    /// Local name reference.
    Name(String),
    /// Provided/required port reference.
    Port(PortReference),
    /// Field access, for example `(required cost).total`.
    FieldAccess {
        /// Base expression.
        base: Box<Expression>,
        /// Field name.
        field: String,
    },
    /// Function call.
    Call {
        /// Function name.
        function: String,
        /// Positional arguments.
        arguments: Vec<Expression>,
    },
    /// Unary expression.
    Unary {
        /// Unary operator.
        operator: UnaryOperator,
        /// Operand.
        operand: Box<Expression>,
    },
    /// Binary expression.
    Binary {
        /// Binary operator.
        operator: BinaryOperator,
        /// Left operand.
        left: Box<Expression>,
        /// Right operand.
        right: Box<Expression>,
    },
    /// Aggregate shorthand, for example `sum mass required by *`.
    Aggregate(AggregateExpression),
    /// Raw fallback when the expression parser cannot safely structure the text.
    Raw(String),
}

/// Provided/required port reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortReference {
    /// Port direction.
    pub direction: PortDirection,
    /// Port name.
    pub port: String,
    /// Optional instance name after `by`.
    pub instance: Option<String>,
}

impl PortReference {
    /// Creates a port reference.
    #[must_use]
    pub fn new(
        direction: PortDirection,
        port: impl Into<String>,
        instance: Option<String>,
    ) -> Self {
        Self {
            direction,
            port: port.into(),
            instance,
        }
    }
}

/// Aggregate expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateExpression {
    /// Aggregate operator.
    pub operator: AggregateOperator,
    /// Port being aggregated.
    pub port: String,
    /// Direction being aggregated.
    pub direction: PortDirection,
    /// Optional instance selector. `None` means wildcard.
    pub instance: Option<String>,
}

/// Aggregate operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateOperator {
    /// Sum aggregate.
    Sum,
}

/// Unary operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    /// Numeric negation.
    Neg,
}

/// Binary operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
}

/// Parses an expression. Returns `Expression::Raw` when the text is outside the
/// supported subset.
#[must_use]
pub fn parse_expression_text(text: &str) -> Expression {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Expression::Raw(String::new());
    }

    let tokens = lex(trimmed);
    let mut parser = Parser::new(&tokens, trimmed);
    match parser.parse_expression() {
        Some(expression) if parser.is_at_end() => expression,
        _ => Expression::Raw(trimmed.to_owned()),
    }
}

/// Parses a comma-separated list of expressions.
#[must_use]
pub fn parse_expression_list_text(text: &str) -> Vec<Expression> {
    split_top_level(text, ',')
        .into_iter()
        .map(|part| parse_expression_text(&part))
        .collect()
}

/// Parses a unit expression. Returns `UnitExpression::Raw` for unsupported units.
#[must_use]
pub fn parse_unit_expression_text(text: &str) -> UnitExpression {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return UnitExpression::One;
    }

    let tokens = lex(trimmed);
    let mut parser = UnitParser::new(&tokens, trimmed);
    match parser.parse_unit_expression() {
        Some(unit) if parser.is_at_end() => unit,
        _ => UnitExpression::Raw(normalize_unit_text(trimmed)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExprTokenKind {
    Word,
    Number,
    Operator,
    Punctuation,
    Backtick,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExprToken {
    kind: ExprTokenKind,
    text: String,
}

impl ExprToken {
    fn new(kind: ExprTokenKind, text: &str, start: usize, end: usize) -> Self {
        Self {
            kind,
            text: text[start..end].to_owned(),
        }
    }
}

fn lex(source: &str) -> Vec<ExprToken> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let kind;
        let mut end = start + ch.len_utf8();

        if ch.is_whitespace() {
            continue;
        }
        if ch == '`' {
            kind = ExprTokenKind::Backtick;
        } else if ch.is_ascii_digit() {
            kind = ExprTokenKind::Number;
            consume_number(&mut chars, &mut end);
        } else if is_word_start(ch) {
            kind = ExprTokenKind::Word;
            consume_while(&mut chars, &mut end, is_word_continue);
        } else if is_operator_char(ch) {
            kind = ExprTokenKind::Operator;
            consume_while(&mut chars, &mut end, is_operator_char);
        } else if is_punctuation(ch) {
            kind = ExprTokenKind::Punctuation;
        } else {
            kind = ExprTokenKind::Word;
        }

        tokens.push(ExprToken::new(kind, source, start, end));
    }

    tokens
}

fn consume_number(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, end: &mut usize) {
    let mut seen_exponent = false;
    while let Some((next_index, next)) = chars.peek().copied() {
        if next.is_ascii_digit() || matches!(next, '.' | '_') {
            *end = next_index + next.len_utf8();
            chars.next();
            continue;
        }
        if !seen_exponent && matches!(next, 'e' | 'E') {
            seen_exponent = true;
            *end = next_index + next.len_utf8();
            chars.next();
            if let Some((sign_index, sign)) = chars.peek().copied()
                && matches!(sign, '+' | '-')
            {
                *end = sign_index + sign.len_utf8();
                chars.next();
            }
            continue;
        }
        break;
    }
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

struct Parser<'a> {
    tokens: &'a [ExprToken],
    position: usize,
    source: &'a str,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [ExprToken], source: &'a str) -> Self {
        Self {
            tokens,
            position: 0,
            source,
        }
    }

    fn parse_expression(&mut self) -> Option<Expression> {
        self.parse_additive()
    }

    fn parse_additive(&mut self) -> Option<Expression> {
        let mut expression = self.parse_multiplicative()?;
        loop {
            let operator = if self.consume_text("+") {
                Some(BinaryOperator::Add)
            } else if self.consume_text("-") {
                Some(BinaryOperator::Sub)
            } else {
                None
            };
            let Some(operator) = operator else {
                break;
            };
            let right = self.parse_multiplicative()?;
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(right),
            };
        }
        Some(expression)
    }

    fn parse_multiplicative(&mut self) -> Option<Expression> {
        let mut expression = self.parse_unary()?;
        loop {
            let operator = if self.consume_text("*") || self.consume_text("·") {
                Some(BinaryOperator::Mul)
            } else if self.consume_text("/") {
                Some(BinaryOperator::Div)
            } else {
                None
            };
            let Some(operator) = operator else {
                break;
            };
            let right = self.parse_unary()?;
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(right),
            };
        }
        Some(expression)
    }

    fn parse_unary(&mut self) -> Option<Expression> {
        if self.consume_text("-") {
            let operand = self.parse_unary()?;
            return Some(Expression::Unary {
                operator: UnaryOperator::Neg,
                operand: Box::new(operand),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expression> {
        let mut expression = self.parse_primary()?;
        loop {
            if !self.consume_text(".") {
                break;
            }
            let field = self.consume_word()?;
            expression = Expression::FieldAccess {
                base: Box::new(expression),
                field,
            };
        }
        Some(expression)
    }

    fn parse_primary(&mut self) -> Option<Expression> {
        if self.consume_text("(") {
            let expression = self.parse_expression()?;
            self.expect_text(")")?;
            return Some(expression);
        }
        if self.consume_text("⟨") {
            return self.parse_tuple();
        }
        if self.consume_text("`") {
            return self.parse_symbol_literal();
        }
        if self.peek_text("provided") || self.peek_text("required") {
            return self.parse_prefix_port_reference();
        }
        if self.peek_text("sum") {
            return self.parse_aggregate();
        }
        if self.peek_kind(ExprTokenKind::Number) {
            return self.parse_quantity();
        }
        self.parse_word_primary()
    }

    fn parse_tuple(&mut self) -> Option<Expression> {
        let mut elements = Vec::new();
        if self.consume_text("⟩") {
            return Some(Expression::Literal(LiteralExpression::Tuple(elements)));
        }

        loop {
            elements.push(self.parse_expression()?);
            if self.consume_text("⟩") {
                break;
            }
            self.expect_text(",")?;
        }

        Some(Expression::Literal(LiteralExpression::Tuple(elements)))
    }

    fn parse_symbol_literal(&mut self) -> Option<Expression> {
        let first = self.consume_word()?;
        if self.consume_text(":") {
            let value = self.consume_word()?;
            return Some(Expression::Literal(LiteralExpression::Symbol {
                poset: Some(first),
                value,
            }));
        }
        Some(Expression::Literal(LiteralExpression::Symbol {
            poset: None,
            value: first,
        }))
    }

    fn parse_prefix_port_reference(&mut self) -> Option<Expression> {
        let direction = self.consume_direction()?;
        let port = self.consume_word()?;
        let instance = if self.consume_text("by") {
            self.consume_instance_selector()
        } else {
            None
        };
        Some(Expression::Port(PortReference::new(
            direction, port, instance,
        )))
    }

    fn parse_aggregate(&mut self) -> Option<Expression> {
        self.expect_text("sum")?;
        let port = self.consume_word()?;
        let direction = self.consume_direction()?;
        self.expect_text("by")?;
        let instance = self.consume_instance_selector();
        Some(Expression::Aggregate(AggregateExpression {
            operator: AggregateOperator::Sum,
            port,
            direction,
            instance,
        }))
    }

    fn parse_quantity(&mut self) -> Option<Expression> {
        let magnitude = self.consume_number()?;
        let unit = self.consume_attached_unit();
        Some(Expression::Literal(LiteralExpression::Quantity(
            QuantityLiteral::new(magnitude, unit),
        )))
    }

    fn parse_word_primary(&mut self) -> Option<Expression> {
        let name = self.consume_word()?;
        if name == "true" {
            return Some(Expression::Literal(LiteralExpression::Bool(true)));
        }
        if name == "false" {
            return Some(Expression::Literal(LiteralExpression::Bool(false)));
        }
        if self.consume_text("(") {
            return self.parse_call(name);
        }
        if self.peek_text("provided") || self.peek_text("required") {
            let direction = self.consume_direction()?;
            self.expect_text("by")?;
            let instance = self.consume_instance_selector();
            return Some(Expression::Port(PortReference::new(
                direction, name, instance,
            )));
        }
        Some(Expression::Name(name))
    }

    fn parse_call(&mut self, function: String) -> Option<Expression> {
        let mut arguments = Vec::new();
        if self.consume_text(")") {
            return Some(Expression::Call {
                function,
                arguments,
            });
        }

        loop {
            arguments.push(self.parse_expression()?);
            if self.consume_text(")") {
                break;
            }
            self.expect_text(",")?;
        }

        Some(Expression::Call {
            function,
            arguments,
        })
    }

    fn consume_attached_unit(&mut self) -> Option<UnitExpression> {
        if self.consume_text("[") {
            let start = self.position;
            let mut depth = 1usize;
            while self.position < self.tokens.len() {
                if self.peek_text("[") {
                    depth += 1;
                } else if self.peek_text("]") {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let text = self.tokens_text(start, self.position);
                        self.position += 1;
                        return Some(parse_unit_expression_text(&text));
                    }
                }
                self.position += 1;
            }
            return Some(UnitExpression::Raw(self.source.to_owned()));
        }

        if !self.peek_unit_atom() {
            return None;
        }
        let start = self.position;
        self.position += 1;
        loop {
            if self.peek_text("^") && self.peek_offset_is_unit_atom(1) {
                self.position += 2;
                continue;
            }
            if (self.peek_text("*") || self.peek_text("/")) && self.peek_offset_is_unit_atom(1) {
                self.position += 2;
                continue;
            }
            break;
        }
        Some(parse_unit_expression_text(
            &self.tokens_text(start, self.position),
        ))
    }

    fn consume_direction(&mut self) -> Option<PortDirection> {
        if self.consume_text("provided") {
            Some(PortDirection::Provides)
        } else if self.consume_text("required") {
            Some(PortDirection::Requires)
        } else {
            None
        }
    }

    fn consume_instance_selector(&mut self) -> Option<String> {
        if self.consume_text("*") {
            return None;
        }
        self.consume_word()
    }

    fn consume_number(&mut self) -> Option<String> {
        if !self.peek_kind(ExprTokenKind::Number) {
            return None;
        }
        let text = self.tokens[self.position].text.clone();
        self.position += 1;
        Some(text)
    }

    fn consume_word(&mut self) -> Option<String> {
        if !self.peek_kind(ExprTokenKind::Word) {
            return None;
        }
        let text = self.tokens[self.position].text.clone();
        self.position += 1;
        Some(text)
    }

    fn expect_text(&mut self, text: &str) -> Option<()> {
        if self.consume_text(text) {
            Some(())
        } else {
            None
        }
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

    fn peek_kind(&self, kind: ExprTokenKind) -> bool {
        self.tokens
            .get(self.position)
            .is_some_and(|token| token.kind == kind)
    }

    fn peek_unit_atom(&self) -> bool {
        self.peek_offset_is_unit_atom(0)
    }

    fn peek_offset_is_unit_atom(&self, offset: usize) -> bool {
        let Some(token) = self.tokens.get(self.position + offset) else {
            return false;
        };
        matches!(token.kind, ExprTokenKind::Word | ExprTokenKind::Number)
            && !matches!(
                token.text.as_str(),
                "provided" | "required" | "by" | "sum" | "constant"
            )
    }

    fn tokens_text(&self, start: usize, end: usize) -> String {
        normalize_unit_text(
            &self.tokens[start..end]
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>(),
        )
    }

    fn is_at_end(&self) -> bool {
        self.position == self.tokens.len()
    }
}

struct UnitParser<'a> {
    tokens: &'a [ExprToken],
    position: usize,
    source: &'a str,
}

impl<'a> UnitParser<'a> {
    fn new(tokens: &'a [ExprToken], source: &'a str) -> Self {
        Self {
            tokens,
            position: 0,
            source,
        }
    }

    fn parse_unit_expression(&mut self) -> Option<UnitExpression> {
        let mut unit = self.parse_unit_power()?;
        loop {
            if self.consume_text("*") {
                let right = self.parse_unit_power()?;
                unit = product_unit(unit, right);
            } else if self.consume_text("/") {
                let denominator = self.parse_unit_power()?;
                unit = UnitExpression::Quotient {
                    numerator: Box::new(unit),
                    denominator: Box::new(denominator),
                };
            } else {
                break;
            }
        }
        Some(unit)
    }

    fn parse_unit_power(&mut self) -> Option<UnitExpression> {
        let mut unit = self.parse_unit_atom()?;
        if self.consume_text("^") {
            let exponent = self.consume_atom_text()?;
            unit = UnitExpression::Power {
                base: Box::new(unit),
                exponent,
            };
        }
        Some(unit)
    }

    fn parse_unit_atom(&mut self) -> Option<UnitExpression> {
        if self.consume_text("(") {
            let unit = self.parse_unit_expression()?;
            self.expect_text(")")?;
            return Some(unit);
        }
        if self.consume_text("`") {
            let symbol = self.consume_atom_text()?;
            return Some(UnitExpression::Symbol(symbol));
        }

        let text = self.consume_atom_text()?;
        if text == "1" {
            return Some(UnitExpression::One);
        }
        if let Some((base, exponent)) = split_superscript_suffix(&text) {
            return Some(UnitExpression::Power {
                base: Box::new(UnitExpression::Symbol(base)),
                exponent,
            });
        }
        Some(UnitExpression::Symbol(text))
    }

    fn consume_atom_text(&mut self) -> Option<String> {
        let token = self.tokens.get(self.position)?;
        if !matches!(token.kind, ExprTokenKind::Word | ExprTokenKind::Number) {
            return None;
        }
        self.position += 1;
        Some(token.text.clone())
    }

    fn expect_text(&mut self, text: &str) -> Option<()> {
        if self.consume_text(text) {
            Some(())
        } else {
            None
        }
    }

    fn consume_text(&mut self, text: &str) -> bool {
        if self
            .tokens
            .get(self.position)
            .is_some_and(|token| token.text == text)
        {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn is_at_end(&self) -> bool {
        self.position == self.tokens.len() || self.source.is_empty()
    }
}

fn product_unit(left: UnitExpression, right: UnitExpression) -> UnitExpression {
    let mut factors = Vec::new();
    match left {
        UnitExpression::Product(existing) => factors.extend(existing),
        UnitExpression::One => {}
        other => factors.push(other),
    }
    match right {
        UnitExpression::Product(existing) => factors.extend(existing),
        UnitExpression::One => {}
        other => factors.push(other),
    }
    match factors.len() {
        0 => UnitExpression::One,
        1 => factors.into_iter().next().unwrap_or(UnitExpression::One),
        _ => UnitExpression::Product(factors),
    }
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

fn normalize_unit_text(text: &str) -> String {
    text.split_whitespace().collect::<String>()
}

fn split_superscript_suffix(text: &str) -> Option<(String, String)> {
    let mut suffix = Vec::new();
    let mut base_end = text.len();
    for (index, ch) in text.char_indices().rev() {
        let Some(digit) = superscript_digit(ch) else {
            break;
        };
        suffix.push(digit);
        base_end = index;
    }
    if suffix.is_empty() || base_end == 0 {
        return None;
    }
    suffix.reverse();
    Some((text[..base_end].to_owned(), suffix.into_iter().collect()))
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

#[derive(Default)]
struct DelimiterDepth {
    parentheses: usize,
    brackets: usize,
    angles: usize,
}

impl DelimiterDepth {
    fn observe(&mut self, ch: char) {
        match ch {
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
        self.parentheses == 0 && self.brackets == 0 && self.angles == 0
    }
}

fn is_word_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic() || ch.is_numeric()
}

fn is_word_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric() || ch.is_numeric()
}

fn is_operator_char(ch: char) -> bool {
    matches!(ch, '+' | '-' | '*' | '/' | '·' | '^')
}

fn is_punctuation(ch: char) -> bool {
    matches!(ch, '(' | ')' | '[' | ']' | ',' | ':' | '.' | '⟨' | '⟩')
}

#[cfg(test)]
mod tests {
    use super::{
        BinaryOperator, Expression, LiteralExpression, PortDirection, UnitExpression,
        parse_expression_list_text, parse_expression_text, parse_unit_expression_text,
    };

    #[test]
    fn parses_quantity_with_compound_unit() {
        let expression = parse_expression_text("25 USD * s / m");

        match expression {
            Expression::Literal(LiteralExpression::Quantity(quantity)) => {
                assert_eq!(quantity.magnitude, "25");
                assert!(matches!(
                    quantity.unit,
                    Some(UnitExpression::Quotient { .. })
                ));
            }
            other => panic!("unexpected expression: {other:?}"),
        }
    }

    #[test]
    fn parses_port_references_and_binary_expression() {
        let expression = parse_expression_text("provided extra_payload + mass required by battery");

        match expression {
            Expression::Binary {
                operator: BinaryOperator::Add,
                left,
                right,
            } => {
                assert!(matches!(*left, Expression::Port(_)));
                match *right {
                    Expression::Port(port) => {
                        assert_eq!(port.direction, PortDirection::Requires);
                        assert_eq!(port.port, "mass");
                        assert_eq!(port.instance.as_deref(), Some("battery"));
                    }
                    other => panic!("unexpected right expression: {other:?}"),
                }
            }
            other => panic!("unexpected expression: {other:?}"),
        }
    }

    #[test]
    fn parses_call_tuple_and_symbolic_value() {
        let call = parse_expression_text("take(provided dyn_prop, 6)");
        assert!(matches!(call, Expression::Call { .. }));

        let tuple = parse_expression_text("⟨12.0 m, `path_type: SuperType⟩");
        match tuple {
            Expression::Literal(LiteralExpression::Tuple(values)) => {
                assert_eq!(values.len(), 2);
            }
            other => panic!("unexpected tuple: {other:?}"),
        }
    }

    #[test]
    fn parses_field_access() {
        let expression = parse_expression_text("(required cost_and_mass).overall_cost");

        assert!(matches!(expression, Expression::FieldAccess { .. }));
    }

    #[test]
    fn parses_catalog_value_lists() {
        let values = parse_expression_list_text("10s, 5Wh");

        assert_eq!(values.len(), 2);
        assert!(matches!(
            values[0],
            Expression::Literal(LiteralExpression::Quantity(_))
        ));
    }

    #[test]
    fn parses_superscript_units() {
        let unit = parse_unit_expression_text("W*s²/m²");

        assert!(matches!(unit, UnitExpression::Quotient { .. }));
    }
}
