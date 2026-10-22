//! Token definitions produced by the lexer.
//!
//! Inline HTML (everything outside `<? ... >`) is carried as its own token so
//! the parser can emit it verbatim in document order.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// Literal text outside any PHP tag, emitted unchanged.
    InlineHtml(String),

    // Literals
    Long(i64),
    Double(f64),
    /// A double-quoted string, kept raw (with escapes) so the interpreter can
    /// interpolate `$variables` at evaluation time.
    Str(String),
    /// A `$name` variable reference.
    Var(String),
    /// A `$name[` array-access opener (the lexer has already consumed `[`).
    ArrayVar(String),

    // Identifiers / keywords (lowercased by the lexer for case-insensitive match)
    Ident(String),

    // Operators & punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Not,     // !
    Assign,  // =
    PlusEq,  // +=
    MinusEq, // -=
    AndEq,   // &=
    OrEq,    // |=
    Inc,     // ++
    Dec,     // --
    EqEq,    // ==
    NotEq,   // !=
    Lt,      // <
    Le,      // <=
    Gt,      // >
    Ge,      // >=
    AndAnd,  // &&
    OrOr,    // ||
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    At, // @ error suppression

    /// Statement terminator: a `;`, a `{`, or the closing `>` of a tag.
    Semi,
    /// A literal `}` (block / brace close).
    RBrace,

    Eof,
}
