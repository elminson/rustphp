//! Hand-written lexer for PHP/FI 1.x.
//!
//! It walks the whole source once, emitting [`Tok::InlineHtml`] for text
//! outside `<? ... >` tags and code tokens inside them. Two quirks of the
//! original `lex.c` are reproduced faithfully:
//!
//! * The closing `>` of a tag doubles as a statement terminator, so `>` is
//!   only treated as a greater-than operator while the lexer is *inside the
//!   parenthesised condition* of an `if` / `elseif` / `while`. Everywhere else
//!   a bare `>` ends the tag. The same gate applies to `<`.
//! * Function and keyword identifiers are case-insensitive, so bare
//!   identifiers are lowercased here; `$variable` names keep their case.

use crate::token::Tok;

pub fn lex(src: &str) -> Result<Vec<Tok>, String> {
    Lexer::new(src).run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    toks: Vec<Tok>,
    /// `Some(depth)` while inside an `if`/`while` condition's parentheses.
    cond_paren: Option<i32>,
    /// Set right after an `if`/`elseif`/`while` keyword; the next `(` opens a
    /// condition.
    expect_cond: bool,
}

impl Lexer {
    fn new(src: &str) -> Self {
        Lexer {
            chars: src.chars().collect(),
            pos: 0,
            toks: Vec::new(),
            cond_paren: None,
            expect_cond: false,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn run(mut self) -> Result<Vec<Tok>, String> {
        loop {
            // --- Outside a tag: copy inline HTML until `<?` / `<!?`. ---
            let mut html = String::new();
            while let Some(c) = self.peek() {
                if c == '<' {
                    let n = self.peek2();
                    if n == Some('?') {
                        self.pos += 2;
                        break;
                    } else if n == Some('!') && self.chars.get(self.pos + 2) == Some(&'?') {
                        self.pos += 3;
                        break;
                    }
                }
                html.push(c);
                self.pos += 1;
            }
            if !html.is_empty() {
                self.toks.push(Tok::InlineHtml(html));
            }
            if self.peek().is_none() {
                break;
            }

            // --- Inside a tag: lex code until END_TAG (`>` at depth 0). ---
            self.cond_paren = None;
            self.expect_cond = false;
            self.lex_code()?;
        }
        self.toks.push(Tok::Eof);
        Ok(self.toks)
    }

    fn lex_code(&mut self) -> Result<(), String> {
        loop {
            let c = match self.peek() {
                Some(c) => c,
                None => return Ok(()), // EOF inside a tag: just stop.
            };

            // Whitespace
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.pos += 1;
                continue;
            }

            // Comment /* ... */
            if c == '/' && self.peek2() == Some('*') {
                self.pos += 2;
                while let Some(d) = self.bump() {
                    if d == '*' && self.peek() == Some('/') {
                        self.pos += 1;
                        break;
                    }
                }
                continue;
            }

            // Variable: $name  (optionally followed by `[`)
            if c == '$' {
                self.pos += 1;
                let name = self.read_ident_raw();
                // Allow whitespace between the name and a `[`.
                let save = self.pos;
                while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
                    self.pos += 1;
                }
                if self.peek() == Some('[') {
                    self.pos += 1;
                    self.toks.push(Tok::ArrayVar(name));
                } else {
                    self.pos = save;
                    self.toks.push(Tok::Var(name));
                }
                continue;
            }

            // Number
            if c.is_ascii_digit() || (c == '.' && self.peek2().map_or(false, |d| d.is_ascii_digit()))
            {
                self.read_number();
                continue;
            }

            // Char literal 'c' -> ASCII code
            if c == '\'' {
                self.pos += 1;
                let text = self.read_quoted('\'');
                let code = text.chars().next().map(|ch| ch as i64).unwrap_or(0);
                self.toks.push(Tok::Long(code));
                continue;
            }

            // Double-quoted string (raw; interpolated at eval time)
            if c == '"' {
                self.pos += 1;
                let text = self.read_quoted('"');
                self.toks.push(Tok::Str(text));
                continue;
            }

            // Identifier / keyword / function name
            if c.is_ascii_alphabetic() || c == '_' {
                let ident = self.read_ident_raw().to_ascii_lowercase();
                self.expect_cond = matches!(ident.as_str(), "if" | "elseif" | "while");
                self.toks.push(Tok::Ident(ident));
                continue;
            }

            // Operators & punctuation
            self.pos += 1;
            match c {
                '+' => match self.peek() {
                    Some('+') => {
                        self.pos += 1;
                        self.toks.push(Tok::Inc);
                    }
                    Some('=') => {
                        self.pos += 1;
                        self.toks.push(Tok::PlusEq);
                    }
                    _ => self.toks.push(Tok::Plus),
                },
                '-' => match self.peek() {
                    Some('-') => {
                        self.pos += 1;
                        self.toks.push(Tok::Dec);
                    }
                    Some('=') => {
                        self.pos += 1;
                        self.toks.push(Tok::MinusEq);
                    }
                    _ => self.toks.push(Tok::Minus),
                },
                '*' => self.toks.push(Tok::Star),
                '/' => self.toks.push(Tok::Slash),
                '%' => self.toks.push(Tok::Percent),
                '^' => self.toks.push(Tok::Caret),
                '&' => match self.peek() {
                    Some('&') => {
                        self.pos += 1;
                        self.toks.push(Tok::AndAnd);
                    }
                    Some('=') => {
                        self.pos += 1;
                        self.toks.push(Tok::AndEq);
                    }
                    _ => self.toks.push(Tok::Amp),
                },
                '|' => match self.peek() {
                    Some('|') => {
                        self.pos += 1;
                        self.toks.push(Tok::OrOr);
                    }
                    Some('=') => {
                        self.pos += 1;
                        self.toks.push(Tok::OrEq);
                    }
                    _ => self.toks.push(Tok::Pipe),
                },
                '!' => {
                    if self.peek() == Some('=') {
                        self.pos += 1;
                        self.toks.push(Tok::NotEq);
                    } else {
                        self.toks.push(Tok::Not);
                    }
                }
                '=' => {
                    if self.peek() == Some('=') {
                        self.pos += 1;
                        self.toks.push(Tok::EqEq);
                    } else {
                        self.toks.push(Tok::Assign);
                    }
                }
                '(' => {
                    if self.expect_cond {
                        self.cond_paren = Some(1);
                        self.expect_cond = false;
                    } else if let Some(d) = self.cond_paren {
                        self.cond_paren = Some(d + 1);
                    }
                    self.toks.push(Tok::LParen);
                }
                ')' => {
                    if let Some(d) = self.cond_paren {
                        let nd = d - 1;
                        self.cond_paren = if nd <= 0 { None } else { Some(nd) };
                    }
                    self.toks.push(Tok::RParen);
                }
                '[' => self.toks.push(Tok::LBracket),
                ']' => self.toks.push(Tok::RBracket),
                ',' => self.toks.push(Tok::Comma),
                '@' => self.toks.push(Tok::At),
                ';' => self.toks.push(Tok::Semi),
                '{' => self.toks.push(Tok::Semi),
                '}' => self.toks.push(Tok::RBrace),
                '>' => {
                    if self.in_condition() {
                        if self.peek() == Some('=') {
                            self.pos += 1;
                            self.toks.push(Tok::Ge);
                        } else {
                            self.toks.push(Tok::Gt);
                        }
                    } else {
                        // END_TAG: terminate statement and leave code mode.
                        self.toks.push(Tok::Semi);
                        return Ok(());
                    }
                }
                '<' => {
                    if self.in_condition() {
                        if self.peek() == Some('=') {
                            self.pos += 1;
                            self.toks.push(Tok::Le);
                        } else {
                            self.toks.push(Tok::Lt);
                        }
                    } else {
                        self.toks.push(Tok::Lt);
                    }
                }
                other => {
                    return Err(format!("unexpected character '{other}' in PHP code"));
                }
            }
        }
    }

    fn in_condition(&self) -> bool {
        self.cond_paren.is_some()
    }

    fn read_ident_raw(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }

    /// Read characters until the closing quote, honouring `\` escapes. The raw
    /// text (escapes intact) is returned; unescaping happens later.
    fn read_quoted(&mut self, quote: char) -> String {
        let mut s = String::new();
        while let Some(c) = self.bump() {
            if c == '\\' {
                s.push('\\');
                if let Some(n) = self.bump() {
                    s.push(n);
                }
                continue;
            }
            if c == quote {
                break;
            }
            s.push(c);
        }
        s
    }

    fn read_number(&mut self) {
        let start = self.pos;
        let mut is_double = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else if c == '.' && !is_double {
                is_double = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_double {
            self.toks
                .push(Tok::Double(text.parse::<f64>().unwrap_or(0.0)));
        } else {
            self.toks
                .push(Tok::Long(crate::value::parse_c_long(&text)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Tok;

    fn lx(s: &str) -> Vec<Tok> {
        lex(s).expect("lex ok")
    }

    #[test]
    fn simple_echo_and_end_tag() {
        // The closing `>` becomes a statement terminator (Semi).
        assert_eq!(
            lx("<?echo $x>"),
            vec![Tok::Ident("echo".into()), Tok::Var("x".into()), Tok::Semi, Tok::Eof]
        );
    }

    #[test]
    fn inline_html_is_preserved() {
        assert_eq!(
            lx("hi<?echo 1>bye"),
            vec![
                Tok::InlineHtml("hi".into()),
                Tok::Ident("echo".into()),
                Tok::Long(1),
                Tok::Semi,
                Tok::InlineHtml("bye".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn gt_is_comparison_only_inside_a_condition() {
        // Inside if(...) the `>` is a comparison; the trailing `>` ends the tag.
        assert_eq!(
            lx("<?if($a>$b)>"),
            vec![
                Tok::Ident("if".into()),
                Tok::LParen,
                Tok::Var("a".into()),
                Tok::Gt,
                Tok::Var("b".into()),
                Tok::RParen,
                Tok::Semi,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn gt_outside_condition_ends_the_tag() {
        // `echo 1>2` does NOT compare; `>` ends the tag, leaving "2>" as HTML.
        assert_eq!(
            lx("<?echo 1>2>"),
            vec![
                Tok::Ident("echo".into()),
                Tok::Long(1),
                Tok::Semi,
                Tok::InlineHtml("2>".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn array_var_opens_bracket() {
        assert_eq!(
            lx("<?$a[1]>"),
            vec![Tok::ArrayVar("a".into()), Tok::Long(1), Tok::RBracket, Tok::Semi, Tok::Eof]
        );
    }

    #[test]
    fn strings_numbers_comments() {
        assert_eq!(
            lx("<?/* skip */echo \"a b\", 3.5>"),
            vec![
                Tok::Ident("echo".into()),
                Tok::Str("a b".into()),
                Tok::Comma,
                Tok::Double(3.5),
                Tok::Semi,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            lx("<?$a += 1 && $b == 2>"),
            vec![
                Tok::Var("a".into()),
                Tok::PlusEq,
                Tok::Long(1),
                Tok::AndAnd,
                Tok::Var("b".into()),
                Tok::EqEq,
                Tok::Long(2),
                Tok::Semi,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn char_literal_is_ascii_code() {
        assert_eq!(lx("<?echo 'A'>"), vec![Tok::Ident("echo".into()), Tok::Long(65), Tok::Semi, Tok::Eof]);
    }
}
