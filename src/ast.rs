//! Abstract syntax tree for PHP/FI 1.x.
//!
//! The original interpreter was a streaming stack machine with no explicit
//! tree; this clean reimplementation parses the same surface syntax into a
//! conventional AST that a tree-walker then evaluates.

#[derive(Clone, Debug)]
pub enum Node {
    /// Literal text emitted verbatim.
    InlineHtml(String),
    /// `echo [fmt] expr [, expr]*` — `fmt` is an optional printf-style string.
    Echo { fmt: Option<Expr>, args: Vec<Expr> },
    Assign { target: Target, op: AssignOp, value: Expr },
    /// `$x++` / `$x--` used as a statement.
    IncDec { target: Target, inc: bool },
    If { branches: Vec<Branch> },
    While { cond: Expr, body: Vec<Node> },
    Switch { subject: Expr, cases: Vec<Case> },
    Break,
    Include(Expr),
    Exit,
    Header(Expr),
    Return(Option<Expr>),
    Global(Vec<String>),
    Static { name: String, value: Option<Expr> },
    FuncDef { name: String, params: Vec<String>, body: Vec<Node> },
    /// A bare expression evaluated for its side effects (e.g. a call).
    ExprStmt(Expr),
}

/// One arm of an if/elseif/else chain; `cond == None` is the `else`.
#[derive(Clone, Debug)]
pub struct Branch {
    pub cond: Option<Expr>,
    pub body: Vec<Node>,
}

/// One `case`/`default` of a switch; `label == None` is `default`.
#[derive(Clone, Debug)]
pub struct Case {
    pub label: Option<Expr>,
    pub body: Vec<Node>,
}

/// The left-hand side of an assignment or increment.
#[derive(Clone, Debug)]
pub enum Target {
    Var(String),
    /// `$a[idx]`; `idx == None` means append (`$a[] = ...`).
    Elem(String, Option<Box<Expr>>),
}

#[derive(Clone, Copy, Debug)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    And,
    Or,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Long(i64),
    Double(f64),
    /// Raw double-quoted string; `$var` interpolation happens at eval time.
    Str(String),
    Var(String),
    Elem(String, Box<Expr>),
    Unary { op: UnOp, expr: Box<Expr> },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    /// `$x++` / `$x--` used in expression position (value is the new value).
    IncDec { target: Target, inc: bool },
    /// Any function call: builtin, special form, or user-defined.
    Call { name: String, args: Vec<Expr>, suppress: bool },
}

#[derive(Clone, Copy, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}
