//! Shared unit-label normalization helpers.
//!
//! These helpers intentionally stay small: they canonicalize the syntactic unit
//! labels used by the finite solver, catalog decoder, and compiler checks.
//! Equivalence checks are dimension-aware, but they do not scale magnitudes.

use std::collections::BTreeMap;

/// Removes insignificant whitespace from a unit expression.
#[must_use]
pub fn normalize_unit_text(text: &str) -> String {
    text.split_whitespace().collect::<String>()
}

/// Canonicalizes a unit label into the stable finite-solver spelling.
///
/// Unitless aliases such as `dimensionless`, `unitless`, `Reals`, and `Real`
/// normalize to `None`.
#[must_use]
pub fn canonical_unit_label(unit: &str) -> Option<String> {
    canonical_unit_option(Some(unit))
}

/// Canonicalizes an optional unit label.
#[must_use]
pub fn canonical_unit_option(unit: Option<&str>) -> Option<String> {
    combine_units(&[(unit, 1)], label_unit_atom_factors)
}

/// Returns true when two unit labels have the same canonical finite-solver form.
#[must_use]
pub fn units_equivalent(left: Option<&str>, right: Option<&str>) -> bool {
    unit_dimension_option(left) == unit_dimension_option(right)
}

fn unit_dimension_option(unit: Option<&str>) -> Option<String> {
    combine_units(&[(unit, 1)], dimension_unit_atom_factors)
}

fn combine_units(
    parts: &[(Option<&str>, i32)],
    atom_factors: fn(&str) -> Vec<(String, i32)>,
) -> Option<String> {
    let mut factors = BTreeMap::<String, i32>::new();
    for (unit, sign) in parts {
        if let Some(unit) = unit {
            apply_unit_factors(unit, *sign, &mut factors, atom_factors);
        }
    }
    unit_label_from_factors(&factors)
}

fn apply_unit_factors(
    unit: &str,
    sign: i32,
    factors: &mut BTreeMap<String, i32>,
    atom_factors: fn(&str) -> Vec<(String, i32)>,
) {
    let mut operator = sign;
    let mut current = String::new();
    for ch in normalize_unit_text(unit).chars() {
        match ch {
            '*' | '/' => {
                apply_unit_factor(&current, operator, factors, atom_factors);
                current.clear();
                operator = if ch == '*' { sign } else { -sign };
            }
            _ => current.push(ch),
        }
    }
    apply_unit_factor(&current, operator, factors, atom_factors);
}

fn apply_unit_factor(
    raw: &str,
    sign: i32,
    factors: &mut BTreeMap<String, i32>,
    atom_factors: fn(&str) -> Vec<(String, i32)>,
) {
    let token = raw.trim();
    if token.is_empty() || matches!(token, "1" | "dimensionless" | "unitless" | "Reals" | "Real") {
        return;
    }

    let (base, exponent) = unit_base_and_exponent(token);
    for (unit, base_exponent) in atom_factors(&base) {
        if unit.is_empty() {
            continue;
        }
        let total = factors.entry(unit).or_default();
        *total += sign * exponent * base_exponent;
        if *total == 0 {
            factors.retain(|_, exponent| *exponent != 0);
        }
    }
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

fn label_unit_atom_factors(atom: &str) -> Vec<(String, i32)> {
    match atom.trim() {
        "$" => vec![("USD".to_owned(), 1)],
        other => vec![(other.to_owned(), 1)],
    }
}

fn dimension_unit_atom_factors(atom: &str) -> Vec<(String, i32)> {
    match atom.trim() {
        "$" => vec![("USD".to_owned(), 1)],
        "km" => vec![("m".to_owned(), 1)],
        "N" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 1),
            ("s".to_owned(), -2),
        ],
        "Nm" | "J" | "kJ" | "Wh" | "kWh" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -2),
        ],
        "W" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -3),
        ],
        "Hz" | "Bq" => vec![("s".to_owned(), -1)],
        "Pa" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), -1),
            ("s".to_owned(), -2),
        ],
        "C" => vec![("A".to_owned(), 1), ("s".to_owned(), 1)],
        "V" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -3),
            ("A".to_owned(), -1),
        ],
        "F" => vec![
            ("kg".to_owned(), -1),
            ("m".to_owned(), -2),
            ("s".to_owned(), 4),
            ("A".to_owned(), 2),
        ],
        "Ω" | "Ohm" | "ohm" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -3),
            ("A".to_owned(), -2),
        ],
        "S" => vec![
            ("kg".to_owned(), -1),
            ("m".to_owned(), -2),
            ("s".to_owned(), 3),
            ("A".to_owned(), 2),
        ],
        "Wb" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -2),
            ("A".to_owned(), -1),
        ],
        "T" => vec![
            ("kg".to_owned(), 1),
            ("s".to_owned(), -2),
            ("A".to_owned(), -1),
        ],
        "H" => vec![
            ("kg".to_owned(), 1),
            ("m".to_owned(), 2),
            ("s".to_owned(), -2),
            ("A".to_owned(), -2),
        ],
        "lm" => vec![("cd".to_owned(), 1)],
        "lx" => vec![("cd".to_owned(), 1), ("m".to_owned(), -2)],
        "Gy" | "Sv" => vec![("m".to_owned(), 2), ("s".to_owned(), -2)],
        "kat" => vec![("mol".to_owned(), 1), ("s".to_owned(), -1)],
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

fn unit_label_from_factors(factors: &BTreeMap<String, i32>) -> Option<String> {
    let numerator = unit_factor_labels(factors, true);
    let denominator = unit_factor_labels(factors, false);
    match (numerator.is_empty(), denominator.is_empty()) {
        (true, true) => None,
        (false, true) => Some(numerator.join("*")),
        (true, false) => Some(format!("1/{}", denominator.join("/"))),
        (false, false) => Some(format!("{}/{}", numerator.join("*"), denominator.join("/"))),
    }
}

fn unit_factor_labels(factors: &BTreeMap<String, i32>, positive: bool) -> Vec<String> {
    factors
        .iter()
        .filter_map(|(unit, exponent)| {
            let exponent = if positive { *exponent } else { -*exponent };
            (exponent > 0).then(|| unit_factor_label(unit, exponent))
        })
        .collect()
}

fn unit_factor_label(unit: &str, exponent: i32) -> String {
    if exponent == 1 {
        unit.to_owned()
    } else {
        format!("{unit}^{exponent}")
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_unit_label, units_equivalent};

    #[test]
    fn canonicalizes_unitless_aliases() {
        assert_eq!(canonical_unit_label("dimensionless"), None);
        assert_eq!(canonical_unit_label("Reals"), None);
        assert_eq!(canonical_unit_label("unitless"), None);
    }

    #[test]
    fn canonicalizes_superscript_and_caret_exponents() {
        assert_eq!(
            canonical_unit_label("W*s²/m²"),
            Some("W*s^2/m^2".to_owned())
        );
        assert!(units_equivalent(Some("m/s²"), Some("m/s^2")));
    }

    #[test]
    fn keeps_multiple_denominator_factors_unambiguous() {
        assert_eq!(
            canonical_unit_label("W*s/m/kg"),
            Some("W*s/kg/m".to_owned())
        );
        assert!(units_equivalent(Some("W*s/m/kg"), Some("W*s/kg/m")));
    }

    #[test]
    fn canonicalizes_currency_alias() {
        assert!(units_equivalent(Some("$"), Some("USD")));
    }

    #[test]
    fn equates_scaled_unit_dimensions_without_relabeling() {
        assert_eq!(canonical_unit_label("deg"), Some("deg".to_owned()));
        assert_eq!(canonical_unit_label("g"), Some("g".to_owned()));
        assert_eq!(canonical_unit_label("km"), Some("km".to_owned()));
        assert!(units_equivalent(Some("deg"), Some("rad")));
        assert!(units_equivalent(Some("Wh"), Some("J")));
        assert!(units_equivalent(Some("g"), Some("kg")));
        assert!(units_equivalent(Some("km"), Some("m")));
        assert!(units_equivalent(Some("min"), Some("s")));
    }

    #[test]
    fn equates_si_derived_unit_dimensions() {
        assert!(units_equivalent(Some("N"), Some("kg*m/s^2")));
        assert!(units_equivalent(Some("Nm"), Some("J")));
        assert!(units_equivalent(Some("W"), Some("J/s")));
        assert!(units_equivalent(Some("Hz"), Some("1/s")));
        assert!(units_equivalent(Some("Pa"), Some("N/m^2")));
        assert!(units_equivalent(Some("C"), Some("A*s")));
        assert!(units_equivalent(Some("V"), Some("W/A")));
        assert!(units_equivalent(Some("F"), Some("C/V")));
        assert!(units_equivalent(Some("Ω"), Some("V/A")));
        assert!(units_equivalent(Some("Ohm"), Some("Ω")));
        assert!(units_equivalent(Some("S"), Some("A/V")));
        assert!(units_equivalent(Some("Wb"), Some("V*s")));
        assert!(units_equivalent(Some("T"), Some("Wb/m^2")));
        assert!(units_equivalent(Some("H"), Some("Wb/A")));
        assert!(units_equivalent(Some("lx"), Some("lm/m^2")));
        assert!(units_equivalent(Some("Bq"), Some("1/s")));
        assert!(units_equivalent(Some("Gy"), Some("J/kg")));
        assert!(units_equivalent(Some("Sv"), Some("J/kg")));
        assert!(units_equivalent(Some("kat"), Some("mol/s")));
    }
}
