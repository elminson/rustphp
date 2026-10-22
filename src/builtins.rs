//! Stateless built-in functions, `printf`-style formatting, and calendar math.
//!
//! Stateful builtins (array cursors, `strtok`, `rand`, dates relative to "now")
//! live in the interpreter; everything here is a pure value transformation.

use crate::value::{format_double, Value};

/// Dispatch a pure builtin by (lowercased) name. Returns `Ok(None)` when the
/// name is not a pure builtin, so the caller can try user functions.
pub fn call(name: &str, a: &[Value]) -> Result<Option<Value>, String> {
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Str(String::new()));
    let v = match name {
        // ---- strings ----
        "strlen" => Value::Long(arg(0).to_str().len() as i64),
        "strtoupper" => Value::Str(arg(0).to_str().to_ascii_uppercase()),
        "strtolower" => Value::Str(arg(0).to_str().to_ascii_lowercase()),
        "ucfirst" => {
            let s = arg(0).to_str();
            let mut c = s.chars();
            Value::Str(match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => s,
            })
        }
        "substr" => Value::Str(substr(&arg(0).to_str(), arg(1).to_long(), a.get(2).map(|v| v.to_long()))),
        // strstr and strchr are identical in PHP/FI.
        "strstr" | "strchr" => {
            let hay = arg(0).to_str();
            let needle = arg(1).to_str();
            match hay.find(&needle) {
                Some(i) => Value::Str(hay[i..].to_string()),
                None => Value::Str(String::new()),
            }
        }
        "strrchr" => {
            let hay = arg(0).to_str();
            let needle = arg(1).to_str();
            // Like C strrchr: search for the last occurrence of the first char.
            match needle.chars().next() {
                Some(ch) => match hay.rfind(ch) {
                    Some(i) => Value::Str(hay[i..].to_string()),
                    None => Value::Str(String::new()),
                },
                None => Value::Str(String::new()),
            }
        }
        "sprintf" => Value::Str(sprintf(&arg(0).to_str(), &a[1.min(a.len())..])),
        "htmlspecialchars" => Value::Str(html_special_chars(&arg(0).to_str())),
        "urlencode" => Value::Str(url_encode(&arg(0).to_str())),
        "quotemeta" => Value::Str(quote_meta(&arg(0).to_str())),

        // ---- math ----
        "abs" => match arg(0) {
            Value::Double(d) => Value::Double(d.abs()),
            other => Value::Long(other.to_long().abs()),
        },
        "sin" => Value::Double(arg(0).to_double().sin()),
        "cos" => Value::Double(arg(0).to_double().cos()),
        "tan" => Value::Double(arg(0).to_double().tan()),
        "exp" => Value::Double(arg(0).to_double().exp()),
        "log" => Value::Double(arg(0).to_double().ln()),
        "log10" => Value::Double(arg(0).to_double().log10()),
        "sqrt" => Value::Double(arg(0).to_double().sqrt()),
        "max" => fold_minmax(a, true),
        "min" => fold_minmax(a, false),

        // ---- type conversion ----
        "intval" => Value::Long(arg(0).to_long()),
        "doubleval" => Value::Double(arg(0).to_double()),
        "strval" => Value::Str(arg(0).to_str()),
        "gettype" => Value::Str(arg(0).type_name().to_string()),
        "bindec" => Value::Long(i64::from_str_radix(&arg(0).to_str(), 2).unwrap_or(0)),
        "octdec" => Value::Long(i64::from_str_radix(&arg(0).to_str(), 8).unwrap_or(0)),
        "hexdec" => Value::Long(i64::from_str_radix(arg(0).to_str().trim_start_matches("0x"), 16).unwrap_or(0)),
        "decbin" => Value::Str(format!("{:b}", arg(0).to_long())),
        "decoct" => Value::Str(format!("{:o}", arg(0).to_long())),
        "dechex" => Value::Str(format!("{:x}", arg(0).to_long())),
        "ord" => Value::Long(arg(0).to_str().bytes().next().unwrap_or(0) as i64),
        "chr" => {
            let b = (arg(0).to_long() & 0xff) as u8;
            Value::Str((b as char).to_string())
        }

        _ => return Ok(None),
    };
    Ok(Some(v))
}

fn fold_minmax(a: &[Value], want_max: bool) -> Value {
    let mut best: Option<Value> = None;
    for v in a {
        best = Some(match best {
            None => v.clone(),
            Some(cur) => {
                let gt = v.to_double() > cur.to_double();
                if gt == want_max {
                    v.clone()
                } else {
                    cur
                }
            }
        });
    }
    best.unwrap_or(Value::Long(0))
}

/// PHP/FI `substr`: negative start counts from the end; negative length trims
/// from the end.
pub fn substr(s: &str, start: i64, len: Option<i64>) -> String {
    let bytes: Vec<char> = s.chars().collect();
    let n = bytes.len() as i64;
    let mut begin = if start < 0 { (n + start).max(0) } else { start.min(n) };
    if begin < 0 {
        begin = 0;
    }
    let end = match len {
        None => n,
        Some(l) if l < 0 => (n + l).max(begin),
        Some(l) => (begin + l).min(n),
    };
    if begin >= end {
        return String::new();
    }
    bytes[begin as usize..end as usize].iter().collect()
}

fn html_special_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn quote_meta(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, '.' | '\\' | '+' | '*' | '?' | '[' | '^' | ']' | '$' | '(' | ')') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// --------------------------------------------------------------------------
// printf-style formatting (a practical C subset: flags - 0 space +, width,
// .precision, and conversions d i u f F e E g G s x X o c %).
// --------------------------------------------------------------------------

pub fn sprintf(fmt: &str, args: &[Value]) -> String {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut argi = 0;
    let next = |argi: &mut usize| -> Value {
        let v = args.get(*argi).cloned().unwrap_or(Value::Str(String::new()));
        *argi += 1;
        v
    };
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < chars.len() && chars[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        // flags
        let (mut left, mut zero, mut plus, mut space) = (false, false, false, false);
        while i < chars.len() {
            match chars[i] {
                '-' => left = true,
                '0' => zero = true,
                '+' => plus = true,
                ' ' => space = true,
                _ => break,
            }
            i += 1;
        }
        // width
        let mut width = 0usize;
        let mut has_width = false;
        while i < chars.len() && chars[i].is_ascii_digit() {
            has_width = true;
            width = width * 10 + (chars[i] as usize - '0' as usize);
            i += 1;
        }
        // precision
        let mut prec: Option<usize> = None;
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let mut p = 0usize;
            while i < chars.len() && chars[i].is_ascii_digit() {
                p = p * 10 + (chars[i] as usize - '0' as usize);
                i += 1;
            }
            prec = Some(p);
        }
        if i >= chars.len() {
            break;
        }
        let conv = chars[i];
        i += 1;
        let body = match conv {
            'd' | 'i' | 'u' => {
                let n = next(&mut argi).to_long();
                let mut s = n.abs().to_string();
                let sign = if n < 0 {
                    "-"
                } else if plus {
                    "+"
                } else if space {
                    " "
                } else {
                    ""
                };
                // zero padding goes between sign and digits
                if zero && !left && has_width && width > sign.len() + s.len() {
                    s = format!("{}{}", "0".repeat(width - sign.len() - s.len()), s);
                }
                format!("{sign}{s}")
            }
            'f' | 'F' => {
                let x = next(&mut argi).to_double();
                let p = prec.unwrap_or(6);
                let mut s = format!("{:.*}", p, x.abs());
                let sign = if x.is_sign_negative() && x != 0.0 {
                    "-"
                } else if plus {
                    "+"
                } else if space {
                    " "
                } else {
                    ""
                };
                if zero && !left && has_width && width > sign.len() + s.len() {
                    s = format!("{}{}", "0".repeat(width - sign.len() - s.len()), s);
                }
                format!("{sign}{s}")
            }
            'e' | 'E' => {
                let x = next(&mut argi).to_double();
                let p = prec.unwrap_or(6);
                let s = format!("{:.*e}", p, x);
                if conv == 'E' {
                    s.to_ascii_uppercase()
                } else {
                    s
                }
            }
            'g' | 'G' => format_double(next(&mut argi).to_double()),
            's' => {
                let mut s = next(&mut argi).to_str();
                if let Some(p) = prec {
                    s.truncate(p);
                }
                s
            }
            'x' => format!("{:x}", next(&mut argi).to_long()),
            'X' => format!("{:X}", next(&mut argi).to_long()),
            'o' => format!("{:o}", next(&mut argi).to_long()),
            'c' => {
                let b = (next(&mut argi).to_long() & 0xff) as u8;
                (b as char).to_string()
            }
            other => {
                out.push('%');
                out.push(other);
                continue;
            }
        };
        out.push_str(&pad(&body, width, left, false));
    }
    out
}

fn pad(body: &str, width: usize, left: bool, _zero: bool) -> String {
    if body.chars().count() >= width {
        return body.to_string();
    }
    let fill = " ".repeat(width - body.chars().count());
    if left {
        format!("{body}{fill}")
    } else {
        format!("{fill}{body}")
    }
}

// --------------------------------------------------------------------------
// Calendar math (proleptic Gregorian, UTC). Used by date()/gmdate()/mktime().
// --------------------------------------------------------------------------

const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Broken-down UTC time.
pub struct Tm {
    pub year: i64,
    pub month: u32, // 1-12
    pub day: u32,   // 1-31
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub wday: u32, // 0=Sunday
    pub yday: u32, // 0-based day of year
}

/// Convert a Unix timestamp to broken-down UTC time (Hinnant's civil_from_days).
pub fn gmtime(ts: i64) -> Tm {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let (hour, min, sec) = ((secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32);
    // wday: 1970-01-01 was a Thursday (=4).
    let wday = (((days % 7) + 7 + 4) % 7) as u32;

    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };

    let yday = day_of_year(year, month, day);
    Tm {
        year,
        month,
        day,
        hour,
        min,
        sec,
        wday,
        yday,
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn day_of_year(y: i64, m: u32, d: u32) -> u32 {
    const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut n = CUM[(m - 1) as usize] + (d - 1);
    if m > 2 && is_leap(y) {
        n += 1;
    }
    n
}

/// days_from_civil (Hinnant): days since Unix epoch for a Y/M/D.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Format a timestamp with a PHP/FI date() format string.
/// Supported letters: y z U Y M m D d H h i s (others pass through literally).
pub fn format_date(fmt: &str, ts: i64) -> String {
    let t = gmtime(ts);
    let mut out = String::new();
    for c in fmt.chars() {
        match c {
            'Y' => out.push_str(&format!("{:04}", t.year)),
            'y' => out.push_str(&format!("{:02}", t.year.rem_euclid(100))),
            'm' => out.push_str(&format!("{:02}", t.month)),
            'n' => out.push_str(&t.month.to_string()),
            'd' => out.push_str(&format!("{:02}", t.day)),
            'j' => out.push_str(&t.day.to_string()),
            'H' => out.push_str(&format!("{:02}", t.hour)),
            'h' => {
                let h12 = if t.hour % 12 == 0 { 12 } else { t.hour % 12 };
                out.push_str(&format!("{:02}", h12));
            }
            'i' => out.push_str(&format!("{:02}", t.min)),
            's' => out.push_str(&format!("{:02}", t.sec)),
            'z' => out.push_str(&t.yday.to_string()),
            'U' => out.push_str(&ts.to_string()),
            'D' => out.push_str(DAYS_SHORT[t.wday as usize]),
            'M' => out.push_str(MONTHS_SHORT[(t.month - 1) as usize]),
            'w' => out.push_str(&t.wday.to_string()),
            'A' => out.push_str(if t.hour < 12 { "AM" } else { "PM" }),
            'a' => out.push_str(if t.hour < 12 { "am" } else { "pm" }),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn substr_variants() {
        assert_eq!(substr("example", 0, Some(3)), "exa");
        assert_eq!(substr("hello", -2, None), "lo");
        assert_eq!(substr("hello", 1, Some(-1)), "ell");
    }

    #[test]
    fn sprintf_basics() {
        assert_eq!(sprintf("%05d", &[Value::Long(42)]), "00042");
        assert_eq!(sprintf("%.2f", &[Value::Double(3.14159)]), "3.14");
        assert_eq!(sprintf("%7.2f", &[Value::Double(10.0 / 3.0)]), "   3.33");
        assert_eq!(sprintf("%x/%s", &[Value::Long(255), s("hi")]), "ff/hi");
        assert_eq!(sprintf("100%%", &[]), "100%");
    }

    #[test]
    fn pure_builtins() {
        assert_eq!(call("strlen", &[s("hello")]).unwrap(), Some(Value::Long(5)));
        assert_eq!(call("strtoupper", &[s("abc")]).unwrap(), Some(s("ABC")));
        assert_eq!(call("strstr", &[s("an example"), s("ex")]).unwrap(), Some(s("example")));
        assert_eq!(call("hexdec", &[s("ff")]).unwrap(), Some(Value::Long(255)));
        assert_eq!(call("dechex", &[Value::Long(255)]).unwrap(), Some(s("ff")));
        assert_eq!(call("bindec", &[s("101")]).unwrap(), Some(Value::Long(5)));
        assert_eq!(call("max", &[Value::Long(3), Value::Long(9), Value::Long(1)]).unwrap(), Some(Value::Long(9)));
        assert!(call("no_such_builtin", &[]).unwrap().is_none());
    }

    #[test]
    fn calendar_is_utc_and_deterministic() {
        assert_eq!(format_date("Y-m-d", 0), "1970-01-01");
        assert_eq!(format_date("H:i:s", 0), "00:00:00");
        assert_eq!(format_date("D", 0), "Thu"); // 1970-01-01 was a Thursday
        // mktime round-trips the epoch.
        assert_eq!(days_from_civil(1970, 1, 1) * 86400, 0);
        let t = gmtime(0);
        assert_eq!((t.year, t.month, t.day), (1970, 1, 1));
    }
}
