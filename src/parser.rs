//! Recursive-descent parser for PHP/FI 1.x.
//!
//! The source language terminates statements with `;`, `{`, or the closing
//! `>` of a tag (all surfaced by the lexer as [`Tok::Semi`]), and closes
//! compound statements with `endif` / `endwhile` / `endswitch` rather than
//! braces. Operator precedence follows C; comparisons and logical operators
//! are accepted in any expression (a harmless superset of the original, which
//! only allowed them inside conditions).

use crate::ast::*;
use crate::token::Tok;

/// Keywords that close a compound statement. Block parsing always stops at one
/// of these so an enclosing construct can see it — PHP/FI lets an `if`/`while`/
/// `switch` be closed implicitly (e.g. at end of file) when its `endX` keyword
/// is missing.
const TERMS: &[&str] = &[
    "elseif",
    "else",
    "endif",
    "endwhile",
    "case",
    "default",
    "endswitch",
];

pub fn parse(toks: Vec<Tok>) -> Result<Vec<Node>, String> {
    let mut p = Parser { toks, pos: 0 };
    let mut nodes = Vec::new();
    loop {
        nodes.extend(p.parse_block()?);
        match p.peek() {
            Tok::Eof => break,
            // A stray closer (unbalanced endif, `}`, …): ignore it, like the
            // original's forgiving stack machine, and keep parsing.
            Tok::Ident(_) | Tok::RBrace | Tok::RParen => p.pos += 1,
            other => return Err(format!("unexpected trailing token: {other:?}")),
        }
    }
    Ok(nodes)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).unwrap_or(&Tok::Eof)
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks.get(self.pos).cloned().unwrap_or(Tok::Eof);
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: Tok) -> Result<(), String> {
        if self.eat(&t) {
            Ok(())
        } else {
            Err(format!("expected {:?}, found {:?}", t, self.peek()))
        }
    }
    fn skip_semis(&mut self) {
        while matches!(self.peek(), Tok::Semi) {
            self.pos += 1;
        }
    }
    fn is_keyword(&self, kws: &[&str]) -> bool {
        matches!(self.peek(), Tok::Ident(k) if kws.contains(&k.as_str()))
    }

    // ----- blocks & statements -----

    fn parse_block(&mut self) -> Result<Vec<Node>, String> {
        let mut nodes = Vec::new();
        loop {
            self.skip_semis();
            match self.peek() {
                Tok::Eof | Tok::RBrace | Tok::RParen => break,
                Tok::Ident(k) if TERMS.contains(&k.as_str()) => break,
                _ => nodes.push(self.parse_statement()?),
            }
        }
        Ok(nodes)
    }

    fn parse_statement(&mut self) -> Result<Node, String> {
        match self.peek().clone() {
            Tok::InlineHtml(s) => {
                self.pos += 1;
                Ok(Node::InlineHtml(s))
            }
            Tok::Var(_) | Tok::ArrayVar(_) => self.parse_assign_or_expr(),
            Tok::Ident(k) => match k.as_str() {
                "echo" => self.parse_echo(),
                "if" => self.parse_if(),
                "while" => self.parse_while(),
                "switch" => self.parse_switch(),
                "break" => {
                    self.pos += 1;
                    Ok(Node::Break)
                }
                "include" => {
                    self.pos += 1;
                    Ok(Node::Include(self.parse_expr()?))
                }
                "exit" => {
                    self.pos += 1;
                    Ok(Node::Exit)
                }
                "header" => {
                    self.pos += 1;
                    Ok(Node::Header(self.parse_expr()?))
                }
                "return" => self.parse_return(),
                "global" => self.parse_global(),
                "static" => self.parse_static(),
                "function" => self.parse_funcdef(),
                _ => Ok(Node::ExprStmt(self.parse_expr()?)),
            },
            _ => Ok(Node::ExprStmt(self.parse_expr()?)),
        }
    }

    fn parse_assign_or_expr(&mut self) -> Result<Node, String> {
        let target = self.parse_target()?;
        let op = match self.peek() {
            Tok::Assign => Some(AssignOp::Set),
            Tok::PlusEq => Some(AssignOp::Add),
            Tok::MinusEq => Some(AssignOp::Sub),
            Tok::AndEq => Some(AssignOp::And),
            Tok::OrEq => Some(AssignOp::Or),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let value = self.parse_expr()?;
            return Ok(Node::Assign { target, op, value });
        }
        match self.peek() {
            Tok::Inc => {
                self.pos += 1;
                Ok(Node::IncDec { target, inc: true })
            }
            Tok::Dec => {
                self.pos += 1;
                Ok(Node::IncDec { target, inc: false })
            }
            _ => {
                // Bare lvalue used as an expression statement.
                let e = match target {
                    Target::Var(n) => Expr::Var(n),
                    Target::Elem(n, Some(i)) => Expr::Elem(n, i),
                    Target::Elem(_, None) => {
                        return Err("cannot use append `$a[]` as a value".into())
                    }
                };
                Ok(Node::ExprStmt(e))
            }
        }
    }

    fn parse_target(&mut self) -> Result<Target, String> {
        match self.bump() {
            Tok::Var(name) => Ok(Target::Var(name)),
            Tok::ArrayVar(name) => {
                if self.eat(&Tok::RBracket) {
                    Ok(Target::Elem(name, None))
                } else {
                    let idx = self.parse_expr()?;
                    self.expect(Tok::RBracket)?;
                    Ok(Target::Elem(name, Some(Box::new(idx))))
                }
            }
            other => Err(format!("expected a variable, found {other:?}")),
        }
    }

    fn parse_echo(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'echo'
        let first = self.parse_expr()?;
        // Format form: a string literal immediately followed by another expr.
        if matches!(first, Expr::Str(_)) && self.starts_expr() {
            let mut args = vec![self.parse_expr()?];
            while self.eat(&Tok::Comma) {
                args.push(self.parse_expr()?);
            }
            return Ok(Node::Echo {
                fmt: Some(first),
                args,
            });
        }
        let mut args = vec![first];
        while self.eat(&Tok::Comma) {
            args.push(self.parse_expr()?);
        }
        Ok(Node::Echo { fmt: None, args })
    }

    fn parse_if(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'if'
        self.expect(Tok::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(Tok::RParen)?;
        self.skip_semis();
        let mut branches = vec![Branch {
            cond: Some(cond),
            body: self.parse_block()?,
        }];
        loop {
            if self.is_keyword(&["elseif"]) {
                self.pos += 1;
                self.expect(Tok::LParen)?;
                let c = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                self.skip_semis();
                branches.push(Branch {
                    cond: Some(c),
                    body: self.parse_block()?,
                });
            } else if self.is_keyword(&["else"]) {
                self.pos += 1;
                self.skip_semis();
                branches.push(Branch {
                    cond: None,
                    body: self.parse_block()?,
                });
            } else if self.is_keyword(&["endif"]) {
                self.pos += 1;
                break;
            } else {
                // No matching `endif` (e.g. end of file): close implicitly,
                // leaving the token for the enclosing construct.
                break;
            }
        }
        Ok(Node::If { branches })
    }

    fn parse_while(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'while'
        self.expect(Tok::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(Tok::RParen)?;
        self.skip_semis();
        let body = self.parse_block()?;
        // Consume a matching `endwhile`; tolerate its absence (implicit close).
        if self.is_keyword(&["endwhile"]) {
            self.pos += 1;
        }
        Ok(Node::While { cond, body })
    }

    fn parse_switch(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'switch'
        self.expect(Tok::LParen)?;
        let subject = self.parse_expr()?;
        self.expect(Tok::RParen)?;
        self.skip_semis();
        let mut cases = Vec::new();
        loop {
            self.skip_semis();
            if self.is_keyword(&["case"]) {
                self.pos += 1;
                let label = self.parse_expr()?;
                self.skip_semis();
                let body = self.parse_block()?;
                cases.push(Case {
                    label: Some(label),
                    body,
                });
            } else if self.is_keyword(&["default"]) {
                self.pos += 1;
                self.skip_semis();
                let body = self.parse_block()?;
                cases.push(Case { label: None, body });
            } else if self.is_keyword(&["endswitch"]) {
                self.pos += 1;
                break;
            } else {
                // Missing `endswitch` (e.g. end of file): close implicitly.
                break;
            }
        }
        Ok(Node::Switch { subject, cases })
    }

    fn parse_return(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'return'
        if self.starts_expr() {
            Ok(Node::Return(Some(self.parse_expr()?)))
        } else {
            Ok(Node::Return(None))
        }
    }

    fn parse_global(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'global'
        let mut names = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::Var(n) => {
                    self.pos += 1;
                    names.push(n);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok(Node::Global(names))
    }

    fn parse_static(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'static'
        let name = match self.bump() {
            Tok::Var(n) => n,
            other => return Err(format!("expected a variable after static, found {other:?}")),
        };
        let value = if self.eat(&Tok::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Node::Static { name, value })
    }

    fn parse_funcdef(&mut self) -> Result<Node, String> {
        self.pos += 1; // 'function'
        let name = match self.bump() {
            Tok::Ident(n) => n,
            other => return Err(format!("expected a function name, found {other:?}")),
        };
        let mut params = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::Var(n) => {
                    self.pos += 1;
                    params.push(n);
                }
                Tok::Comma => {
                    self.pos += 1;
                }
                _ => break,
            }
        }
        self.expect(Tok::LParen)?;
        let body = self.parse_block()?;
        self.expect(Tok::RParen)?;
        Ok(Node::FuncDef { name, params, body })
    }

    // ----- expressions -----

    fn starts_expr(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Long(_)
                | Tok::Double(_)
                | Tok::Str(_)
                | Tok::Var(_)
                | Tok::ArrayVar(_)
                | Tok::Ident(_)
                | Tok::LParen
                | Tok::Minus
                | Tok::Not
                | Tok::At
        )
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_bin(0)
    }

    fn parse_bin(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, p) = match self.peek_binop() {
                Some(x) => x,
                None => break,
            };
            let lbp = 2 * p;
            if lbp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_bin(lbp + 1)?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn peek_binop(&self) -> Option<(BinOp, u8)> {
        Some(match self.peek() {
            Tok::OrOr => (BinOp::Or, 1),
            Tok::AndAnd => (BinOp::And, 2),
            Tok::Pipe => (BinOp::BitOr, 3),
            Tok::Caret => (BinOp::BitXor, 4),
            Tok::Amp => (BinOp::BitAnd, 5),
            Tok::EqEq => (BinOp::Eq, 6),
            Tok::NotEq => (BinOp::Ne, 6),
            Tok::Lt => (BinOp::Lt, 7),
            Tok::Le => (BinOp::Le, 7),
            Tok::Gt => (BinOp::Gt, 7),
            Tok::Ge => (BinOp::Ge, 7),
            Tok::Plus => (BinOp::Add, 8),
            Tok::Minus => (BinOp::Sub, 8),
            Tok::Star => (BinOp::Mul, 9),
            Tok::Slash => (BinOp::Div, 9),
            Tok::Percent => (BinOp::Mod, 9),
            _ => return None,
        })
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Tok::Not => {
                self.pos += 1;
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            Tok::Minus => {
                self.pos += 1;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            Tok::At => {
                self.pos += 1;
                let inner = self.parse_unary()?;
                Ok(match inner {
                    Expr::Call { name, args, .. } => Expr::Call {
                        name,
                        args,
                        suppress: true,
                    },
                    other => other,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek() {
                Tok::Inc | Tok::Dec => {
                    let inc = matches!(self.peek(), Tok::Inc);
                    if let Some(target) = expr_to_target(&e) {
                        self.pos += 1;
                        e = Expr::IncDec { target, inc };
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Tok::Long(n) => Ok(Expr::Long(n)),
            Tok::Double(d) => Ok(Expr::Double(d)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Var(n) => Ok(Expr::Var(n)),
            Tok::ArrayVar(n) => {
                if self.eat(&Tok::RBracket) {
                    Err(format!("`${n}[]` has no value; it is only valid as an assignment target"))
                } else {
                    let idx = self.parse_expr()?;
                    self.expect(Tok::RBracket)?;
                    Ok(Expr::Elem(n, Box::new(idx)))
                }
            }
            Tok::LParen => {
                let e = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Tok::Ident(name) => {
                self.expect(Tok::LParen)?;
                let mut args = Vec::new();
                if !matches!(self.peek(), Tok::RParen) {
                    args.push(self.parse_expr()?);
                    while self.eat(&Tok::Comma) {
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call {
                    name,
                    args,
                    suppress: false,
                })
            }
            other => Err(format!("unexpected token in expression: {other:?}")),
        }
    }
}

fn expr_to_target(e: &Expr) -> Option<Target> {
    match e {
        Expr::Var(n) => Some(Target::Var(n.clone())),
        Expr::Elem(n, i) => Some(Target::Elem(n.clone(), Some(i.clone()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn prog(s: &str) -> Vec<Node> {
        parse(lex(s).unwrap()).expect("parse ok")
    }

    #[test]
    fn echo_format_vs_plain() {
        // A string literal with NO following comma is a printf format.
        match &prog("<?echo \"%d\" 5>")[0] {
            Node::Echo { fmt: Some(_), args } => assert_eq!(args.len(), 1),
            other => panic!("expected format echo, got {other:?}"),
        }
        // With a comma it is plain multi-argument echo.
        match &prog("<?echo \"x\", 5>")[0] {
            Node::Echo { fmt: None, args } => assert_eq!(args.len(), 2),
            other => panic!("expected plain echo, got {other:?}"),
        }
    }

    #[test]
    fn if_elseif_else_branches() {
        match &prog("<?if($a);echo 1;elseif($b);echo 2;else;echo 3;endif>")[0] {
            Node::If { branches } => {
                assert_eq!(branches.len(), 3);
                assert!(branches[0].cond.is_some());
                assert!(branches[1].cond.is_some());
                assert!(branches[2].cond.is_none()); // else
            }
            other => panic!("expected if, got {other:?}"),
        }
    }

    #[test]
    fn assignment_operators() {
        let p = prog("<?$a = 1; $a += 2; $b[0] = 3; $c[] = 4>");
        assert!(matches!(&p[0], Node::Assign { op: AssignOp::Set, .. }));
        assert!(matches!(&p[1], Node::Assign { op: AssignOp::Add, .. }));
        assert!(matches!(&p[2], Node::Assign { target: Target::Elem(_, Some(_)), .. }));
        assert!(matches!(&p[3], Node::Assign { target: Target::Elem(_, None), .. }));
    }

    #[test]
    fn switch_cases_and_default() {
        match &prog("<?switch($a);case 1;echo 1;break;default;echo 0;endswitch>")[0] {
            Node::Switch { cases, .. } => {
                assert_eq!(cases.len(), 2);
                assert!(cases[0].label.is_some());
                assert!(cases[1].label.is_none());
            }
            other => panic!("expected switch, got {other:?}"),
        }
    }

    #[test]
    fn function_definition_params() {
        match &prog("<?Function Sum $a,$b (return($a+$b);)>")[0] {
            Node::FuncDef { name, params, .. } => {
                assert_eq!(name, "sum"); // names are lowercased
                assert_eq!(params, &vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected funcdef, got {other:?}"),
        }
    }

    #[test]
    fn unclosed_if_at_eof_is_accepted() {
        // PHP/FI accepts an if left open at end of file.
        let p = prog("<?if($a);echo 1>");
        assert!(matches!(&p[0], Node::If { .. }));
    }

    #[test]
    fn precedence_multiplication_binds_tighter() {
        match &prog("<?echo 1+2*3>")[0] {
            Node::Echo { args, .. } => match &args[0] {
                Expr::Binary { op: BinOp::Add, rhs, .. } => {
                    assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
                }
                other => panic!("expected add at top, got {other:?}"),
            },
            _ => panic!("expected echo"),
        }
    }
}
