//! The PHP/FI scalar value type and its tri-type coercions.
//!
//! PHP/FI 1.x has exactly three scalar types: `LNUMBER` (a C `long`),
//! `DNUMBER` (a C `double`) and `STRING`. The original stored all three
//! representations side by side in every variable and converted lazily; we
//! keep a single canonical representation and coerce on demand, which is
//! observably equivalent.
//!
//! Truthiness mirrors `CheckCond()` in the C source (`cond.c`): a number is
//! true when non-zero, a string is true when non-empty. Note that the string
//! `"0"` is therefore **true** in PHP/FI, unlike modern PHP.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Long(i64),
    Double(f64),
    Str(String),
}

impl Value {
    /// Coerce to a C-style `long` (mirrors `atol()` on strings).
    pub fn to_long(&self) -> i64 {
        match self {
            Value::Long(n) => *n,
            Value::Double(d) => *d as i64,
            Value::Str(s) => parse_c_long(s),
        }
    }

    /// Coerce to a `double` (mirrors `atof()` on strings).
    pub fn to_double(&self) -> f64 {
        match self {
            Value::Long(n) => *n as f64,
            Value::Double(d) => *d,
            Value::Str(s) => parse_c_double(s),
        }
    }

    /// Coerce to a display string, as the default (format-less) `echo` would.
    pub fn to_str(&self) -> String {
        match self {
            Value::Long(n) => n.to_string(),
            Value::Double(d) => format_double(*d),
            Value::Str(s) => s.clone(),
        }
    }

    /// Truthiness, per `CheckCond()`: non-zero number or non-empty string.
    pub fn to_bool(&self) -> bool {
        match self {
            Value::Long(n) => *n != 0,
            Value::Double(d) => *d != 0.0,
            Value::Str(s) => !s.is_empty(),
        }
    }

    /// True when this value is a numeric type (not a string).
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Long(_) | Value::Double(_))
    }

    /// A short type name, as `gettype()` reports it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Long(_) => "integer",
            Value::Double(_) => "double",
            Value::Str(_) => "string",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_str())
    }
}

/// Format a double the way the interpreter prints it with no format string.
/// Integers-as-doubles print without a trailing `.0` to match PHP/FI output.
pub fn format_double(d: f64) -> String {
    if d.is_finite() && d.fract() == 0.0 && d.abs() < 1e15 {
        format!("{}", d as i64)
    } else {
        // Shortest round-trippable form; good enough for format-less echo.
        let s = format!("{d}");
        s
    }
}

/// C `atol`: optional sign then leading decimal digits; junk after is ignored.
pub fn parse_c_long(s: &str) -> i64 {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_whitespace() {
        i += 1;
    }
    let mut sign: i64 = 1;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        if b[i] == b'-' {
            sign = -1;
        }
        i += 1;
    }
    let mut n: i64 = 0;
    let mut saw = false;
    while i < b.len() && b[i].is_ascii_digit() {
        n = n.saturating_mul(10).saturating_add((b[i] - b'0') as i64);
        i += 1;
        saw = true;
    }
    if saw {
        sign * n
    } else {
        0
    }
}

/// C `atof`: parse the longest leading floating-point prefix, else 0.0.
pub fn parse_c_double(s: &str) -> f64 {
    let trimmed = s.trim_start();
    let b = trimmed.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let mut saw_exp = false;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            saw_exp = true;
        }
        if saw_exp {
            i = j;
        }
    }
    trimmed[..i].parse::<f64>().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coercions() {
        assert_eq!(Value::Long(42).to_str(), "42");
        assert_eq!(Value::Str("42abc".into()).to_long(), 42);
        assert_eq!(Value::Str("  -7".into()).to_long(), -7);
        assert_eq!(Value::Str("abc".into()).to_long(), 0);
        assert_eq!(Value::Double(3.0).to_long(), 3);
        assert_eq!(Value::Str("3.14xyz".into()).to_double(), 3.14);
        assert_eq!(Value::Str("".into()).to_double(), 0.0);
    }

    #[test]
    fn truthiness_matches_checkcond() {
        // Numbers: non-zero is true.
        assert!(Value::Long(1).to_bool());
        assert!(!Value::Long(0).to_bool());
        assert!(!Value::Double(0.0).to_bool());
        // Strings: non-empty is true, so "0" is TRUE (unlike modern PHP).
        assert!(Value::Str("0".into()).to_bool());
        assert!(!Value::Str("".into()).to_bool());
    }

    #[test]
    fn double_formatting() {
        assert_eq!(format_double(5.0), "5");
        assert_eq!(format_double(3.5), "3.5");
    }

    #[test]
    fn c_number_parsers() {
        assert_eq!(parse_c_long("007"), 7);
        assert_eq!(parse_c_long("+9x"), 9);
        assert_eq!(parse_c_double("-2.5e2junk"), -250.0);
        assert_eq!(parse_c_double("nope"), 0.0);
    }
}
