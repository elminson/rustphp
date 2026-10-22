//! Tree-walking interpreter for PHP/FI 1.x.
//!
//! The original was a streaming stack machine; this evaluator walks the AST
//! but reproduces the same observable semantics: three scalar types with lazy
//! coercion, Perl-style associative arrays with an internal cursor, non
//! short-circuiting logical operators, C-style `switch` fall-through, and
//! function-local variable scope with `global` / `static` opt-outs.
//!
//! Runtime problems (division by zero, calls to undefined/unsupported
//! functions, bad includes) are *soft*: they print a warning when error
//! reporting is on and evaluation continues with an empty value, mirroring the
//! original's `Error()` behaviour and the `@` suppression operator.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::*;
use crate::builtins;
use crate::lexer::lex;
use crate::parser::parse;
use crate::value::Value;

#[derive(Clone)]
enum VarVal {
    Scalar(Value),
    Array(ArrayVal),
}

#[derive(Clone, Default)]
struct ArrayVal {
    order: Vec<String>,
    map: HashMap<String, Value>,
    cursor: usize,
}

#[derive(Default)]
struct Frame {
    vars: HashMap<String, VarVal>,
    globals: HashSet<String>,
    statics: HashMap<String, String>, // local name -> statics storage key
    func: Option<String>,
}

enum Flow {
    Normal,
    Break,
    Return(Value),
}

/// Functions a real PHP/FI build offers but this port does not implement;
/// calling one yields a clear message rather than "undefined function".
const UNSUPPORTED: &[&str] = &[
    "msql", "fopen", "fclose", "fgets", "fputs", "fgetss", "fseek", "ftell", "rewind", "popen",
    "pclose", "file", "feof", "exec", "system", "crypt", "sleep", "usleep", "chdir", "chmod",
    "chown", "chgrp", "mkdir", "unlink", "rename", "opendir", "readdir", "closedir", "rewinddir",
    "tempnam", "fsockopen", "gethostbyname", "gethostbyaddr", "reg_match", "reg_search",
    "reg_replace", "escapeshellcmd", "securevar", "eval", "dbmopen", "dbmclose", "dbmfetch",
    "dbminsert", "imagecreate", "imagegif",
];

pub struct Interp {
    frames: Vec<Frame>,
    functions: HashMap<String, (Vec<String>, Vec<Node>)>,
    statics: HashMap<String, VarVal>,
    pub out: String,
    pub headers: Vec<String>,
    exiting: bool,
    error_print: bool,
    rng: u64,
    strtok_buf: Vec<char>,
    strtok_delims: String,
    strtok_pos: usize,
    include_dir: PathBuf,
}

impl Interp {
    pub fn new(include_dir: PathBuf) -> Self {
        Interp {
            frames: vec![Frame::default()],
            functions: HashMap::new(),
            statics: HashMap::new(),
            out: String::new(),
            headers: Vec::new(),
            exiting: false,
            error_print: true,
            rng: 1,
            strtok_buf: Vec::new(),
            strtok_delims: String::new(),
            strtok_pos: 0,
            include_dir,
        }
    }

    /// Seed a variable in the global scope (used for CGI/CLI-supplied vars).
    pub fn set_global(&mut self, name: &str, value: Value) {
        self.frames[0]
            .vars
            .insert(name.to_string(), VarVal::Scalar(value));
    }

    pub fn run(&mut self, program: &[Node]) {
        // Hoist function definitions so calls can precede definitions, matching
        // nothing in the original (which required definition-first) but being a
        // harmless convenience; definitions also register as they execute.
        for node in program {
            if let Node::FuncDef { name, params, body } = node {
                self.functions
                    .insert(name.clone(), (params.clone(), body.clone()));
            }
        }
        self.exec_block(program);
    }

    // ----- statement execution -----

    fn exec_block(&mut self, block: &[Node]) -> Flow {
        for node in block {
            if self.exiting {
                return Flow::Normal;
            }
            match self.exec_node(node) {
                Flow::Normal => {}
                other => return other,
            }
        }
        Flow::Normal
    }

    fn exec_node(&mut self, node: &Node) -> Flow {
        match node {
            Node::InlineHtml(s) => self.out.push_str(s),
            Node::Echo { fmt, args } => {
                let vals: Vec<Value> = args.iter().map(|e| self.eval(e)).collect();
                match fmt {
                    Some(f) => {
                        let spec = self.eval(f).to_str();
                        let s = builtins::sprintf(&spec, &vals);
                        self.out.push_str(&s);
                    }
                    None => {
                        for v in &vals {
                            self.out.push_str(&v.to_str());
                        }
                    }
                }
            }
            Node::Assign { target, op, value } => {
                let rhs = self.eval(value);
                let new = match op {
                    AssignOp::Set => rhs,
                    AssignOp::Add => self.binop(BinOp::Add, self.read_target(target), rhs),
                    AssignOp::Sub => self.binop(BinOp::Sub, self.read_target(target), rhs),
                    AssignOp::And => {
                        Value::Long(self.read_target(target).to_long() & rhs.to_long())
                    }
                    AssignOp::Or => {
                        Value::Long(self.read_target(target).to_long() | rhs.to_long())
                    }
                };
                self.write_target(target, new);
            }
            Node::IncDec { target, inc } => {
                let cur = self.read_target(target);
                let new = self.binop(if *inc { BinOp::Add } else { BinOp::Sub }, cur, Value::Long(1));
                self.write_target(target, new);
            }
            Node::If { branches } => {
                for b in branches {
                    let take = match &b.cond {
                        None => true,
                        Some(c) => self.eval(c).to_bool(),
                    };
                    if take {
                        return self.exec_block(&b.body);
                    }
                }
            }
            Node::While { cond, body } => loop {
                if self.exiting || !self.eval(cond).to_bool() {
                    break;
                }
                match self.exec_block(body) {
                    Flow::Break => break,
                    Flow::Return(v) => return Flow::Return(v),
                    Flow::Normal => {}
                }
            },
            Node::Switch { subject, cases } => return self.exec_switch(subject, cases),
            Node::Break => return Flow::Break,
            Node::Include(e) => return self.exec_include(e),
            Node::Exit => self.exiting = true,
            Node::Header(e) => {
                let h = self.eval(e).to_str();
                self.headers.push(h);
            }
            Node::Return(opt) => {
                let v = opt.as_ref().map(|e| self.eval(e)).unwrap_or(Value::Str(String::new()));
                return Flow::Return(v);
            }
            Node::Global(names) => {
                for n in names {
                    if let Some(f) = self.frames.last_mut() {
                        f.globals.insert(n.clone());
                    }
                }
            }
            Node::Static { name, value } => {
                let func = self
                    .frames
                    .last()
                    .and_then(|f| f.func.clone())
                    .unwrap_or_default();
                let key = format!("{func}::{name}");
                if !self.statics.contains_key(&key) {
                    let init = value
                        .as_ref()
                        .map(|e| self.eval(e))
                        .unwrap_or(Value::Str(String::new()));
                    self.statics.insert(key.clone(), VarVal::Scalar(init));
                }
                if let Some(f) = self.frames.last_mut() {
                    f.statics.insert(name.clone(), key);
                }
            }
            Node::FuncDef { name, params, body } => {
                self.functions
                    .insert(name.clone(), (params.clone(), body.clone()));
            }
            Node::ExprStmt(e) => {
                self.eval(e);
            }
        }
        Flow::Normal
    }

    fn exec_switch(&mut self, subject: &Expr, cases: &[Case]) -> Flow {
        let subj = self.eval(subject);
        let mut start = None;
        for (i, c) in cases.iter().enumerate() {
            if let Some(label) = &c.label {
                let l = self.eval(label);
                if Self::compare(BinOp::Eq, &subj, &l) {
                    start = Some(i);
                    break;
                }
            }
        }
        if start.is_none() {
            start = cases.iter().position(|c| c.label.is_none());
        }
        if let Some(si) = start {
            for c in &cases[si..] {
                match self.exec_block(&c.body) {
                    Flow::Break => return Flow::Normal,
                    Flow::Return(v) => return Flow::Return(v),
                    Flow::Normal => {}
                }
                if self.exiting {
                    return Flow::Normal;
                }
            }
        }
        Flow::Normal
    }

    fn exec_include(&mut self, e: &Expr) -> Flow {
        let path = self.eval(e).to_str();
        let full = self.include_dir.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(src) => match lex(&src).and_then(parse) {
                Ok(nodes) => {
                    for node in &nodes {
                        if let Node::FuncDef { name, params, body } = node {
                            self.functions
                                .insert(name.clone(), (params.clone(), body.clone()));
                        }
                    }
                    return self.exec_block(&nodes);
                }
                Err(err) => self.soft_error(&format!("include parse error in {path}: {err}")),
            },
            Err(err) => self.soft_error(&format!("include: cannot open {path}: {err}")),
        }
        Flow::Normal
    }

    // ----- expression evaluation -----

    fn eval(&mut self, e: &Expr) -> Value {
        match e {
            Expr::Long(n) => Value::Long(*n),
            Expr::Double(d) => Value::Double(*d),
            Expr::Str(raw) => Value::Str(self.expand_string(raw)),
            Expr::Var(n) => self.get_scalar(n),
            Expr::Elem(n, idx) => {
                let key = self.eval(idx).to_str();
                self.get_elem(n, &key)
            }
            Expr::Unary { op, expr } => {
                let v = self.eval(expr);
                match op {
                    UnOp::Not => Value::Long(if v.to_bool() { 0 } else { 1 }),
                    UnOp::Neg => match v {
                        Value::Double(d) => Value::Double(-d),
                        Value::Long(n) => Value::Long(-n),
                        Value::Str(ref s) => {
                            if is_double_str(s) {
                                Value::Double(-v.to_double())
                            } else {
                                Value::Long(-v.to_long())
                            }
                        }
                    },
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let a = self.eval(lhs);
                let b = self.eval(rhs);
                self.binop(*op, a, b)
            }
            Expr::IncDec { target, inc } => {
                let cur = self.read_target(target);
                let new = self.binop(if *inc { BinOp::Add } else { BinOp::Sub }, cur, Value::Long(1));
                self.write_target(target, new.clone());
                new
            }
            Expr::Call { name, args, suppress } => {
                let old = self.error_print;
                if *suppress {
                    self.error_print = false;
                }
                let v = self.dispatch_call(name, args);
                self.error_print = old;
                v
            }
        }
    }

    fn binop(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        use BinOp::*;
        match op {
            Add | Sub | Mul => {
                if double_ctx(&a) || double_ctx(&b) {
                    let (x, y) = (a.to_double(), b.to_double());
                    Value::Double(match op {
                        Add => x + y,
                        Sub => x - y,
                        _ => x * y,
                    })
                } else {
                    let (x, y) = (a.to_long(), b.to_long());
                    Value::Long(match op {
                        Add => x.wrapping_add(y),
                        Sub => x.wrapping_sub(y),
                        _ => x.wrapping_mul(y),
                    })
                }
            }
            Div => {
                if double_ctx(&a) || double_ctx(&b) {
                    let y = b.to_double();
                    if y == 0.0 {
                        self.soft_error("Division by zero");
                        Value::Long(0)
                    } else {
                        Value::Double(a.to_double() / y)
                    }
                } else {
                    let y = b.to_long();
                    if y == 0 {
                        self.soft_error("Division by zero");
                        Value::Long(0)
                    } else {
                        Value::Long(a.to_long() / y)
                    }
                }
            }
            Mod => {
                let y = b.to_long();
                if y == 0 {
                    self.soft_error("Modulo by zero");
                    Value::Long(0)
                } else {
                    Value::Long(a.to_long() % y)
                }
            }
            BitAnd => Value::Long(a.to_long() & b.to_long()),
            BitOr => Value::Long(a.to_long() | b.to_long()),
            BitXor => Value::Long(a.to_long() ^ b.to_long()),
            And => Value::Long((a.to_bool() && b.to_bool()) as i64),
            Or => Value::Long((a.to_bool() || b.to_bool()) as i64),
            Eq | Ne | Lt | Le | Gt | Ge => Value::Long(Self::compare(op, &a, &b) as i64),
        }
    }

    /// Comparison keyed on the left operand's type, as in the C `Compare()`.
    fn compare(op: BinOp, a: &Value, b: &Value) -> bool {
        use BinOp::*;
        match a {
            Value::Str(sa) => {
                let sb = b.to_str();
                let o = sa.as_str().cmp(sb.as_str());
                match op {
                    Eq => o.is_eq(),
                    Ne => o.is_ne(),
                    Lt => o.is_lt(),
                    Le => o.is_le(),
                    Gt => o.is_gt(),
                    Ge => o.is_ge(),
                    _ => false,
                }
            }
            Value::Double(_) => {
                let (x, y) = (a.to_double(), b.to_double());
                match op {
                    Eq => x == y,
                    Ne => x != y,
                    Lt => x < y,
                    Le => x <= y,
                    Gt => x > y,
                    Ge => x >= y,
                    _ => false,
                }
            }
            Value::Long(_) => {
                let (x, y) = (a.to_long(), b.to_long());
                match op {
                    Eq => x == y,
                    Ne => x != y,
                    Lt => x < y,
                    Le => x <= y,
                    Gt => x > y,
                    Ge => x >= y,
                    _ => false,
                }
            }
        }
    }

    // ----- builtin / function dispatch -----

    fn dispatch_call(&mut self, name: &str, args: &[Expr]) -> Value {
        // Builtins that need a variable *name*, not just its value.
        match name {
            "isset" => return Value::Long(self.is_set(&args[0]) as i64),
            "unset" => {
                self.do_unset(&args[0]);
                return Value::Str(String::new());
            }
            "count" => return Value::Long(self.array_count(arg_var(args))),
            "sort" => {
                self.array_sort(arg_var(args));
                return Value::Long(1);
            }
            "reset" | "end" | "next" | "prev" | "key" => {
                return self.array_cursor(name, arg_var(args));
            }
            "settype" => {
                let ty = self.eval_or_empty(args, 1).to_str();
                let n = arg_var(args).to_string();
                let cur = self.get_scalar(&n);
                self.set_scalar(&n, coerce_type(cur, &ty));
                return Value::Long(1);
            }
            "strtok" => return self.strtok(args),
            _ => {}
        }

        let vals: Vec<Value> = args.iter().map(|e| self.eval(e)).collect();

        match name {
            "time" => return Value::Long(now()),
            "microtime" => {
                let n = now();
                return Value::Str(format!("0.00000000 {n}"));
            }
            "date" => {
                let ts = vals.get(1).map(|v| v.to_long()).unwrap_or_else(now);
                return Value::Str(builtins::format_date(&vals[0].to_str(), ts));
            }
            "gmdate" => {
                let ts = vals.get(1).map(|v| v.to_long()).unwrap_or_else(now);
                return Value::Str(builtins::format_date(&vals[0].to_str(), ts));
            }
            "mktime" => return Value::Long(self.mktime(&vals)),
            "rand" => return Value::Long(self.rand()),
            "srand" => {
                self.rng = vals.get(0).map(|v| v.to_long() as u64).unwrap_or(1).max(1);
                return Value::Long(0);
            }
            "getrandmax" => return Value::Long(32767),
            "phpversion" => return Value::Str("1.99s (rustphp)".into()),
            "phpinfo" => {
                self.phpinfo();
                return Value::Long(1);
            }
            "getenv" => return Value::Str(std::env::var(vals[0].to_str()).unwrap_or_default()),
            "putenv" => {
                if let Some((k, v)) = vals[0].to_str().split_once('=') {
                    std::env::set_var(k, v);
                }
                return Value::Long(1);
            }
            _ => {}
        }

        match builtins::call(name, &vals) {
            Ok(Some(v)) => return v,
            Ok(None) => {}
            Err(msg) => {
                self.soft_error(&msg);
                return Value::Str(String::new());
            }
        }

        if self.functions.contains_key(name) {
            return self.run_user_func(name, vals);
        }
        if UNSUPPORTED.contains(&name) {
            self.soft_error(&format!("function '{name}()' is not supported by rustphp"));
        } else {
            self.soft_error(&format!("call to undefined function '{name}()'"));
        }
        Value::Str(String::new())
    }

    fn run_user_func(&mut self, name: &str, argvals: Vec<Value>) -> Value {
        let (params, body) = match self.functions.get(name) {
            Some(f) => f.clone(),
            None => return Value::Str(String::new()),
        };
        let mut frame = Frame {
            func: Some(name.to_string()),
            ..Frame::default()
        };
        for (i, p) in params.iter().enumerate() {
            let v = argvals.get(i).cloned().unwrap_or(Value::Str(String::new()));
            frame.vars.insert(p.clone(), VarVal::Scalar(v));
        }
        self.frames.push(frame);
        let flow = self.exec_block(&body);
        self.frames.pop();
        match flow {
            Flow::Return(v) => v,
            _ => Value::Str(String::new()),
        }
    }

    // ----- variable storage -----

    fn get_varval(&self, name: &str) -> Option<VarVal> {
        let f = self.frames.last().unwrap();
        if f.globals.contains(name) {
            self.frames[0].vars.get(name).cloned()
        } else if let Some(k) = f.statics.get(name) {
            self.statics.get(k).cloned()
        } else {
            f.vars.get(name).cloned()
        }
    }

    fn resolve_mut(&mut self, name: &str) -> (&mut HashMap<String, VarVal>, String) {
        let (is_global, static_key) = {
            let f = self.frames.last().unwrap();
            (f.globals.contains(name), f.statics.get(name).cloned())
        };
        if is_global {
            (&mut self.frames[0].vars, name.to_string())
        } else if let Some(k) = static_key {
            (&mut self.statics, k)
        } else {
            let idx = self.frames.len() - 1;
            (&mut self.frames[idx].vars, name.to_string())
        }
    }

    fn get_scalar(&self, name: &str) -> Value {
        match self.get_varval(name) {
            Some(VarVal::Scalar(v)) => v,
            _ => Value::Str(String::new()),
        }
    }

    fn get_elem(&self, name: &str, key: &str) -> Value {
        match self.get_varval(name) {
            Some(VarVal::Array(a)) => a.map.get(key).cloned().unwrap_or(Value::Str(String::new())),
            _ => Value::Str(String::new()),
        }
    }

    fn set_scalar(&mut self, name: &str, v: Value) {
        let (m, k) = self.resolve_mut(name);
        m.insert(k, VarVal::Scalar(v));
    }

    fn set_elem(&mut self, name: &str, key: &str, v: Value) {
        let (m, k) = self.resolve_mut(name);
        let entry = m.entry(k).or_insert_with(|| VarVal::Array(ArrayVal::default()));
        if !matches!(entry, VarVal::Array(_)) {
            *entry = VarVal::Array(ArrayVal::default());
        }
        if let VarVal::Array(arr) = entry {
            if !arr.map.contains_key(key) {
                arr.order.push(key.to_string());
            }
            arr.map.insert(key.to_string(), v);
        }
    }

    fn append_elem(&mut self, name: &str, v: Value) {
        let next = match self.get_varval(name) {
            Some(VarVal::Array(a)) => a
                .order
                .iter()
                .filter_map(|k| k.parse::<i64>().ok())
                .max()
                .map(|m| m + 1)
                .unwrap_or(0),
            _ => 0,
        };
        self.set_elem(name, &next.to_string(), v);
    }

    fn read_target(&self, t: &Target) -> Value {
        match t {
            Target::Var(n) => self.get_scalar(n),
            Target::Elem(n, Some(idx)) => {
                // read_target is &self; evaluate index purely (no side effects expected).
                let key = self.eval_pure(idx).to_str();
                self.get_elem(n, &key)
            }
            Target::Elem(_, None) => Value::Str(String::new()),
        }
    }

    fn write_target(&mut self, t: &Target, v: Value) {
        match t {
            Target::Var(n) => self.set_scalar(n, v),
            Target::Elem(n, Some(idx)) => {
                let key = self.eval(idx).to_str();
                let name = n.clone();
                self.set_elem(&name, &key, v);
            }
            Target::Elem(n, None) => {
                let name = n.clone();
                self.append_elem(&name, v);
            }
        }
    }

    /// A side-effect-free index evaluation used by `read_target` (which only
    /// has `&self`). Indexes are almost always literals or simple variables.
    fn eval_pure(&self, e: &Expr) -> Value {
        match e {
            Expr::Long(n) => Value::Long(*n),
            Expr::Double(d) => Value::Double(*d),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Var(n) => self.get_scalar(n),
            Expr::Elem(n, idx) => {
                let key = self.eval_pure(idx).to_str();
                self.get_elem(n, &key)
            }
            _ => Value::Str(String::new()),
        }
    }

    // ----- array helpers -----

    fn array_count(&self, name: &str) -> i64 {
        match self.get_varval(name) {
            Some(VarVal::Array(a)) => a.order.len() as i64,
            Some(VarVal::Scalar(_)) => 1,
            None => 0,
        }
    }

    fn array_sort(&mut self, name: &str) {
        let (m, k) = self.resolve_mut(name);
        if let Some(VarVal::Array(arr)) = m.get_mut(&k) {
            let mut vals: Vec<Value> = arr.order.iter().filter_map(|key| arr.map.get(key).cloned()).collect();
            let all_num = vals.iter().all(|v| v.is_number() || is_double_str(&v.to_str()) || v.to_str().chars().all(|c| c.is_ascii_digit() || c == '-'));
            if all_num {
                vals.sort_by(|a, b| a.to_double().partial_cmp(&b.to_double()).unwrap_or(std::cmp::Ordering::Equal));
            } else {
                vals.sort_by(|a, b| a.to_str().cmp(&b.to_str()));
            }
            let mut new = ArrayVal::default();
            for (i, v) in vals.into_iter().enumerate() {
                let key = i.to_string();
                new.order.push(key.clone());
                new.map.insert(key, v);
            }
            *arr = new;
        }
    }

    fn array_cursor(&mut self, op: &str, name: &str) -> Value {
        let (m, k) = self.resolve_mut(name);
        if let Some(VarVal::Array(arr)) = m.get_mut(&k) {
            let n = arr.order.len();
            match op {
                "reset" => arr.cursor = 0,
                "end" => arr.cursor = n.saturating_sub(1),
                "next" => arr.cursor += 1,
                "prev" => {
                    if arr.cursor == 0 {
                        arr.cursor = n; // past the start
                    } else {
                        arr.cursor -= 1;
                    }
                }
                "key" => {
                    return arr
                        .order
                        .get(arr.cursor)
                        .map(|k| Value::Str(k.clone()))
                        .unwrap_or(Value::Str(String::new()));
                }
                _ => {}
            }
            return arr
                .order
                .get(arr.cursor)
                .and_then(|key| arr.map.get(key).cloned())
                .unwrap_or(Value::Str(String::new()));
        }
        Value::Str(String::new())
    }

    fn is_set(&self, e: &Expr) -> bool {
        match e {
            Expr::Var(n) => self.get_varval(n).is_some(),
            Expr::Elem(n, idx) => {
                let key = self.eval_pure(idx).to_str();
                matches!(self.get_varval(n), Some(VarVal::Array(a)) if a.map.contains_key(&key))
            }
            _ => false,
        }
    }

    fn do_unset(&mut self, e: &Expr) {
        match e {
            Expr::Var(n) => {
                let (m, k) = self.resolve_mut(n);
                m.remove(&k);
            }
            Expr::Elem(n, idx) => {
                let key = self.eval(idx).to_str();
                let name = n.clone();
                let (m, k) = self.resolve_mut(&name);
                if let Some(VarVal::Array(arr)) = m.get_mut(&k) {
                    arr.map.remove(&key);
                    arr.order.retain(|o| o != &key);
                }
            }
            _ => {}
        }
    }

    // ----- misc builtins with interpreter state -----

    fn strtok(&mut self, args: &[Expr]) -> Value {
        let vals: Vec<Value> = args.iter().map(|e| self.eval(e)).collect();
        let delims;
        if vals.len() >= 2 {
            self.strtok_buf = vals[0].to_str().chars().collect();
            self.strtok_pos = 0;
            delims = vals[1].to_str();
            self.strtok_delims = delims.clone();
        } else {
            delims = vals.get(0).map(|v| v.to_str()).unwrap_or_default();
            self.strtok_delims = delims.clone();
        }
        let is_delim = |c: char| delims.contains(c);
        // Skip leading delimiters.
        while self.strtok_pos < self.strtok_buf.len() && is_delim(self.strtok_buf[self.strtok_pos]) {
            self.strtok_pos += 1;
        }
        if self.strtok_pos >= self.strtok_buf.len() {
            return Value::Str(String::new());
        }
        let start = self.strtok_pos;
        while self.strtok_pos < self.strtok_buf.len() && !is_delim(self.strtok_buf[self.strtok_pos]) {
            self.strtok_pos += 1;
        }
        Value::Str(self.strtok_buf[start..self.strtok_pos].iter().collect())
    }

    fn rand(&mut self) -> i64 {
        // Numerical Recipes LCG; take the high bits for a 0..=32767 result.
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        ((self.rng >> 16) & 0x7fff) as i64
    }

    fn mktime(&self, vals: &[Value]) -> i64 {
        let t = builtins::gmtime(now());
        let g = |i: usize, d: i64| vals.get(i).map(|v| v.to_long()).unwrap_or(d);
        let hour = g(0, t.hour as i64);
        let min = g(1, t.min as i64);
        let sec = g(2, t.sec as i64);
        let month = g(3, t.month as i64);
        let day = g(4, t.day as i64);
        let year = g(5, t.year);
        builtins::days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec
    }

    fn eval_or_empty(&mut self, args: &[Expr], i: usize) -> Value {
        match args.get(i) {
            Some(e) => self.eval(e),
            None => Value::Str(String::new()),
        }
    }

    fn phpinfo(&mut self) {
        self.out.push_str(
            "<h1>rustphp</h1>\n\
             <p>A clean-room reimplementation of PHP/FI 1.99s in Rust.</p>\n\
             <table border=1 cellpadding=4>\n",
        );
        let rows = [
            ("PHP/FI version", "1.99s (emulated)"),
            ("Engine", concat!("rustphp ", env!("CARGO_PKG_VERSION"))),
            ("Build", "Rust"),
            ("System", std::env::consts::OS),
            ("Architecture", std::env::consts::ARCH),
        ];
        for (k, v) in rows {
            self.out.push_str(&format!("<tr><td><b>{k}</b></td><td>{v}</td></tr>\n"));
        }
        self.out.push_str("</table>\n");
    }

    // ----- string interpolation -----

    /// Apply escape sequences and interpolate `$var` / `$arr[key]` references
    /// inside a double-quoted string literal.
    fn expand_string(&self, raw: &str) -> String {
        let chars: Vec<char> = raw.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                i += 1;
                if i >= chars.len() {
                    out.push('\\');
                    break;
                }
                match chars[i] {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '$' => out.push('$'),
                    '0' => out.push('\0'),
                    o => {
                        out.push('\\');
                        out.push(o);
                    }
                }
                i += 1;
                continue;
            }
            if c == '$' {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j > start {
                    let name: String = chars[start..j].iter().collect();
                    if j < chars.len() && chars[j] == '[' {
                        let mut k = j + 1;
                        let mut key = String::new();
                        while k < chars.len() && chars[k] != ']' {
                            key.push(chars[k]);
                            k += 1;
                        }
                        let key = self.interp_key(&key);
                        out.push_str(&self.get_elem(&name, &key).to_str());
                        i = if k < chars.len() { k + 1 } else { k };
                    } else {
                        out.push_str(&self.get_scalar(&name).to_str());
                        i = j;
                    }
                    continue;
                }
                out.push('$');
                i += 1;
                continue;
            }
            out.push(c);
            i += 1;
        }
        out
    }

    fn interp_key(&self, key: &str) -> String {
        let k = key.trim();
        if let Some(var) = k.strip_prefix('$') {
            return self.get_scalar(var).to_str();
        }
        k.trim_matches('"').trim_matches('\'').to_string()
    }

    fn soft_error(&mut self, msg: &str) {
        if self.error_print {
            self.out.push_str(&format!("\n<b>Warning</b>: {msg}\n"));
        }
    }
}

fn arg_var(args: &[Expr]) -> &str {
    match args.first() {
        Some(Expr::Var(n)) => n,
        _ => "",
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether a value should drive floating-point arithmetic.
fn double_ctx(v: &Value) -> bool {
    match v {
        Value::Double(_) => true,
        Value::Long(_) => false,
        Value::Str(s) => is_double_str(s),
    }
}

fn is_double_str(s: &str) -> bool {
    let t = s.trim();
    (t.contains('.') || t.contains('e') || t.contains('E'))
        && t.chars().next().map_or(false, |c| {
            c.is_ascii_digit() || c == '-' || c == '+' || c == '.'
        })
}

fn coerce_type(v: Value, ty: &str) -> Value {
    match ty {
        "integer" | "int" => Value::Long(v.to_long()),
        "double" | "real" | "float" => Value::Double(v.to_double()),
        "string" => Value::Str(v.to_str()),
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::Interp;
    use crate::lexer::lex;
    use crate::parser::parse;

    /// Run a PHP/FI source string and return everything it emitted.
    fn run(src: &str) -> String {
        let program = parse(lex(src).expect("lex")).expect("parse");
        let mut interp = Interp::new(std::path::PathBuf::from("."));
        interp.run(&program);
        interp.out
    }

    #[test]
    fn inline_html_passthrough() {
        assert_eq!(run("A<?echo 1>B"), "A1B");
    }

    #[test]
    fn arithmetic_precedence_and_division() {
        assert_eq!(run("<?echo 1+2*3>"), "7");
        assert_eq!(run("<?echo 10/3>"), "3"); // integer division
        assert_eq!(run("<?echo \"%.2f\" 10.0/3>"), "3.33"); // float division
        assert_eq!(run("<?echo 10%3>"), "1");
    }

    #[test]
    fn string_interpolation() {
        assert_eq!(run("<?$x=\"hi\"; echo \"a$x b\">"), "ahi b");
        assert_eq!(run("<?$a[\"k\"]=\"v\"; echo \"[$a[k]]\">"), "[v]");
    }

    #[test]
    fn truthiness_quirk() {
        // String "0" is truthy in PHP/FI.
        assert_eq!(run("<?if(\"0\");echo \"t\";else;echo \"f\";endif>"), "t");
        assert_eq!(run("<?if(0);echo \"t\";else;echo \"f\";endif>"), "f");
    }

    #[test]
    fn while_loop() {
        assert_eq!(run("<?$i=0; while($i<3); echo $i; $i++; endwhile>"), "012");
    }

    #[test]
    fn arrays_and_append_and_count() {
        assert_eq!(run("<?$a[0]=5; $a[\"k\"]=6; $a[]=7; echo $a[0],$a[\"k\"],$a[1]>"), "567");
        assert_eq!(run("<?$a[0]=1;$a[1]=2;$a[2]=3; echo count($a)>"), "3");
    }

    #[test]
    fn array_cursor() {
        assert_eq!(
            run("<?$a[0]=10;$a[1]=20;$a[2]=30; echo reset($a),next($a),end($a)>"),
            "102030"
        );
    }

    #[test]
    fn user_function_and_return() {
        assert_eq!(run("<?Function Sq $x (return($x*$x);); echo Sq(9)>"), "81");
    }

    #[test]
    fn local_scope_is_isolated() {
        // $a in the function is local; the global $a is untouched.
        assert_eq!(run("<?$a=1; Function F (echo $a;); F(); echo $a>"), "1");
    }

    #[test]
    fn global_keyword() {
        assert_eq!(run("<?$a=5; Function F (global $a; echo $a;); F()>"), "5");
    }

    #[test]
    fn static_persists_across_calls() {
        assert_eq!(
            run("<?Function C (static $n=0; $n++; echo $n;); C();C();C()>"),
            "123"
        );
    }

    #[test]
    fn switch_fallthrough_and_break() {
        assert_eq!(run("<?$a=0; switch($a);case 1;echo \"one\";break;case 0;echo \"zero\";break;default;echo \"d\";endswitch>"), "zero");
        assert_eq!(run("<?$a=9; switch($a);case 1;echo \"one\";break;default;echo \"d\";endswitch>"), "d");
    }

    #[test]
    fn comparison_keyed_on_left_type() {
        assert_eq!(run("<?if(5>3);echo \"y\";endif>"), "y");
        assert_eq!(run("<?if(\"abc\"<\"abd\");echo \"y\";endif>"), "y"); // strcmp
    }

    #[test]
    fn logical_not() {
        assert_eq!(run("<?echo !0, !5, !\"\">"), "101");
    }

    #[test]
    fn increment_yields_new_value_immediately() {
        assert_eq!(run("<?$a=5; echo $a++>"), "6");
        assert_eq!(run("<?$a=5; $a+=3; echo $a>"), "8");
    }

    #[test]
    fn strtok_keeps_state() {
        assert_eq!(
            run("<?$t=strtok(\"a b c\",\" \"); while($t); echo $t; $t=strtok(\" \"); endwhile>"),
            "abc"
        );
    }

    #[test]
    fn error_suppression_hides_warning() {
        let out = run("<?echo @nosuchfunc()>");
        assert!(!out.contains("Warning"), "suppressed call should not warn: {out:?}");
    }

    #[test]
    fn unsupported_function_warns_but_continues() {
        let out = run("<?echo msql(\"x\",\"y\"); echo \"after\">");
        assert!(out.contains("not supported"));
        assert!(out.contains("after"));
    }

    #[test]
    fn exit_halts_execution() {
        assert_eq!(run("before<?exit>after"), "before");
    }
}
