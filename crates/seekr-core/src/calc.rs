//! Calculator: evaluate a query that is a mathematical expression (`2+2*3`,
//! `(1+2)^10/7`, `sqrt(2)`, `15% of 80`), like Spotlight does.
//!
//! Anything that parses as arithmetic is answered, dates (`2024-01-05`) and
//! phone numbers (`555-123-4567`) included; the answer is pinned above the
//! search results, which still follow. It takes at least one operator or
//! function and nothing it doesn't understand, so plain numbers (`2024`),
//! versions (`1.2.3`), filenames (`IMG_1234`, `report-2`) and words are not
//! expressions.
//!
//! Grammar, loosest binding first:
//! - `a + b`, `a - b`; `a + b%` and `a - b%` add or take off b percent of a
//! - `a * b`, `a / b`, `a % b` (modulo, floored), also `×`, `÷` and ` x `
//! - `-a`, `+a`, `√a`
//! - `a ^ b` (right associative; also `**`), so `-2^2` is -4
//! - `a!` (factorial), `a%` (percent), `a% of b`
//! - numbers (`1.5`, `.5`, `1e3`, `1,234,567`, `1_000`), `pi`, `tau`, `e`,
//!   `(…)`, and functions `sqrt abs round floor ceil ln log log2 log10 sin
//!   cos tan asin acos atan exp` (radians), always with parentheses.
use serde::{Deserialize, Serialize};
use std::fmt;

/// Significant digits kept in answers: enough for any everyday sum, few
/// enough that float noise (`0.1+0.2` = 0.30000000000000004) never shows.
const SIG_DIGITS: i32 = 14;

/// A query that evaluated to a number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calculation {
    /// What was typed.
    pub input: String,
    /// The expression, normalized: `(1 + 2)^10 / 7`.
    pub expr: String,
    /// The answer, rounded to [`raw`](Self::raw)'s precision.
    pub value: f64,
    /// The answer for copying and pasting: `8435.5714285714`, `1.5e20`.
    pub raw: String,
    /// The answer for reading, with thousands separators: `8,435.5714285714`.
    pub display: String,
}

/// Why a query has no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CalcError {
    /// Not an expression: no operator (`2024`, `pi`), or something that is not
    /// arithmetic (`1.2.3`, `IMG_1234`, a word).
    NotAnExpression,
    /// `1/0`, `5 % 0`.
    DivisionByZero,
    /// Not a finite real number: `sqrt(-1)`, `ln(0)`, `2^5000`, `2.5!`.
    Undefined,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CalcError::NotAnExpression => "not a calculation",
            CalcError::DivisionByZero => "division by zero",
            CalcError::Undefined => "undefined",
        })
    }
}

impl std::error::Error for CalcError {}

/// Evaluate `input` if it is a calculation.
pub fn evaluate(input: &str) -> Result<Calculation, CalcError> {
    let s = input.trim();
    // `=2+2` and `2+2=` are fine too.
    let s = s.strip_prefix('=').unwrap_or(s);
    let s = s.strip_suffix('=').unwrap_or(s).trim();
    let toks = lex(s)?;
    let mut p = Parser { toks: &toks, pos: 0 };
    let node = p.expr()?;
    if p.pos != toks.len() || !node.has_operator() {
        return Err(CalcError::NotAnExpression);
    }
    let v = node.eval()?;
    let raw = format_raw(v);
    Ok(Calculation {
        input: input.to_string(),
        expr: node.to_string(),
        value: raw.parse().unwrap_or(v),
        display: group_thousands(&raw),
        raw,
    })
}

// ---------------------------------------------------------------------------
// Lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A number and its literal, separators removed (`1,234.5` → `1234.5`).
    Num(f64, String),
    Ident(&'static str),
    /// `+ - * / % ^ ! √`, with `×`, `÷`, `−`, `**` and ` x ` mapped to these.
    Op(char),
    Open,
    Close,
}

const CONSTANTS: [(&str, f64); 4] = [
    ("pi", std::f64::consts::PI),
    ("π", std::f64::consts::PI),
    ("tau", std::f64::consts::TAU),
    ("e", std::f64::consts::E),
];

const FUNCTIONS: [&str; 16] = [
    "sqrt", "abs", "round", "floor", "ceil", "ln", "log", "log2", "log10", "sin", "cos", "tan",
    "asin", "acos", "atan", "exp",
];

fn lex(s: &str) -> Result<Vec<Tok>, CalcError> {
    let cs: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let (tok, next) = lex_number(&cs, i)?;
            toks.push(tok);
            i = next;
            continue;
        }
        if c.is_alphabetic() {
            let start = i;
            while i < cs.len() && cs[i].is_alphabetic() {
                i += 1;
            }
            let mut word: String = cs[start..i].iter().collect::<String>().to_lowercase();
            // log2( and log10( are one word.
            if word == "log" {
                for suffix in ["2", "10"] {
                    let n = suffix.len();
                    let digits: String = cs[i..(i + n).min(cs.len())].iter().collect();
                    if digits == suffix && !cs.get(i + n).is_some_and(|d| d.is_ascii_digit()) {
                        word.push_str(suffix);
                        i += n;
                        break;
                    }
                }
            }
            // `3 x 4` multiplies; `1920x1080` or `x2` stay names.
            if word == "x" {
                let spaced = |j: Option<usize>| {
                    j.and_then(|j| cs.get(j)).is_some_and(|c| c.is_whitespace())
                };
                if spaced(start.checked_sub(1)) && spaced(Some(i)) {
                    toks.push(Tok::Op('*'));
                    continue;
                }
                return Err(CalcError::NotAnExpression);
            }
            let known = CONSTANTS
                .iter()
                .map(|(n, _)| *n)
                .chain(FUNCTIONS)
                .chain(["of"])
                .find(|n| *n == word)
                .ok_or(CalcError::NotAnExpression)?;
            toks.push(Tok::Ident(known));
            continue;
        }
        let tok = match c {
            '+' | '-' | '/' | '%' | '^' | '!' | '√' => Tok::Op(c),
            '*' if cs.get(i + 1) == Some(&'*') => {
                i += 1;
                Tok::Op('^')
            }
            '*' | '×' | '·' => Tok::Op('*'),
            '÷' => Tok::Op('/'),
            '−' => Tok::Op('-'),
            '(' => Tok::Open,
            ')' => Tok::Close,
            _ => return Err(CalcError::NotAnExpression),
        };
        toks.push(tok);
        i += 1;
    }
    Ok(toks)
}

/// A number starting at `cs[i]`: digits with optional `,` or `_` thousands
/// separators (strictly grouped by three), a fraction, an exponent.
fn lex_number(cs: &[char], mut i: usize) -> Result<(Tok, usize), CalcError> {
    let mut lit = String::new();
    let mut groups: Vec<usize> = vec![0];
    let mut sep: Option<char> = None;
    while i < cs.len() {
        let c = cs[i];
        if c.is_ascii_digit() {
            lit.push(c);
            *groups.last_mut().unwrap() += 1;
        } else if (c == ',' || c == '_') && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            if sep.is_some_and(|s| s != c) {
                return Err(CalcError::NotAnExpression);
            }
            sep = Some(c);
            groups.push(0);
        } else {
            break;
        }
        i += 1;
    }
    if groups.len() > 1 && (!(1..=3).contains(&groups[0]) || groups[1..].iter().any(|&g| g != 3)) {
        return Err(CalcError::NotAnExpression);
    }
    if cs.get(i) == Some(&'.') {
        if !cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            return Err(CalcError::NotAnExpression);
        }
        lit.push('.');
        i += 1;
        while i < cs.len() && cs[i].is_ascii_digit() {
            lit.push(cs[i]);
            i += 1;
        }
    }
    if matches!(cs.get(i), Some('e' | 'E')) {
        let signed = matches!(cs.get(i + 1), Some('+' | '-'));
        let first = if signed { i + 2 } else { i + 1 };
        if cs.get(first).is_some_and(|d| d.is_ascii_digit()) {
            lit.push('e');
            if signed {
                lit.push(cs[i + 1]);
            }
            i = first;
            while i < cs.len() && cs[i].is_ascii_digit() {
                lit.push(cs[i]);
                i += 1;
            }
        }
    }
    let lit = if lit.starts_with('.') { format!("0{lit}") } else { lit };
    let v: f64 = lit.parse().map_err(|_| CalcError::NotAnExpression)?;
    if !v.is_finite() {
        return Err(CalcError::Undefined);
    }
    Ok((Tok::Num(v, lit), i))
}

// ---------------------------------------------------------------------------
// Parser

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64, String),
    Const(&'static str, f64),
    Neg(Box<Node>),
    Bin(Bin, Box<Node>, Box<Node>),
    Fact(Box<Node>),
    Percent(Box<Node>),
    /// `15% of 80`: the left side is always a [`Node::Percent`].
    Of(Box<Node>, Box<Node>),
    Func(&'static str, Box<Node>),
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
}

type Parsed = Result<Node, CalcError>;

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.pos + k)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Parsed {
        let mut l = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op('+')) => Bin::Add,
                Some(Tok::Op('-')) => Bin::Sub,
                _ => return Ok(l),
            };
            self.pos += 1;
            let r = self.term()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn term(&mut self) -> Parsed {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op('*')) => Bin::Mul,
                Some(Tok::Op('/')) => Bin::Div,
                // A `%` still here is modulo: postfix() took the percent signs.
                Some(Tok::Op('%')) => Bin::Mod,
                // `5(6+7)`, `(1+2)(3+4)`, `2pi`, `2√9`: juxtaposition multiplies.
                // A bare number never follows implicitly (`2 3`, `(555) 123-4567`).
                Some(Tok::Open | Tok::Ident(_) | Tok::Op('√')) if self.operand_at(0) => {
                    let r = self.unary()?;
                    l = Node::Bin(Bin::Mul, Box::new(l), Box::new(r));
                    continue;
                }
                _ => return Ok(l),
            };
            self.pos += 1;
            let r = self.unary()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn unary(&mut self) -> Parsed {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(Node::Neg(Box::new(self.unary()?)))
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            Some(Tok::Op('√')) => {
                self.pos += 1;
                Ok(Node::Func("sqrt", Box::new(self.unary()?)))
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Parsed {
        let base = self.postfix()?;
        if self.eat(&Tok::Op('^')) {
            let exp = self.unary()?;
            return Ok(Node::Bin(Bin::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Parsed {
        let mut n = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::Op('!')) => {
                    self.pos += 1;
                    n = Node::Fact(Box::new(n));
                }
                // `50%` is a percentage unless an operand follows (`10 % 3`).
                Some(Tok::Op('%')) if !self.operand_at(1) => {
                    self.pos += 1;
                    n = Node::Percent(Box::new(n));
                    if self.eat(&Tok::Ident("of")) {
                        let of = self.unary()?;
                        return Ok(Node::Of(Box::new(n), Box::new(of)));
                    }
                }
                _ => return Ok(n),
            }
        }
    }

    /// Whether the token `k` ahead starts an operand (so a `%` before it is modulo).
    fn operand_at(&self, k: usize) -> bool {
        match self.peek_at(k) {
            Some(Tok::Num(..) | Tok::Open | Tok::Op('√')) => true,
            Some(Tok::Ident(w)) => *w != "of",
            _ => false,
        }
    }

    fn primary(&mut self) -> Parsed {
        let t = self.peek().cloned().ok_or(CalcError::NotAnExpression)?;
        self.pos += 1;
        match t {
            Tok::Num(v, lit) => Ok(Node::Num(v, lit)),
            Tok::Open => {
                let n = self.expr()?;
                if !self.eat(&Tok::Close) {
                    return Err(CalcError::NotAnExpression);
                }
                Ok(n)
            }
            Tok::Ident(w) => {
                if let Some((_, v)) = CONSTANTS.iter().find(|(n, _)| *n == w) {
                    return Ok(Node::Const(w, *v));
                }
                if FUNCTIONS.contains(&w) && self.eat(&Tok::Open) {
                    let arg = self.expr()?;
                    if self.eat(&Tok::Close) {
                        return Ok(Node::Func(w, Box::new(arg)));
                    }
                }
                Err(CalcError::NotAnExpression)
            }
            Tok::Op(_) | Tok::Close => Err(CalcError::NotAnExpression),
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluation

fn finite(v: f64) -> Result<f64, CalcError> {
    if v.is_finite() { Ok(v) } else { Err(CalcError::Undefined) }
}

/// A sum that cancels down to rounding error is zero: `0.3 - 0.1 - 0.2`.
fn snap_sum(r: f64, a: f64, b: f64) -> f64 {
    if r.abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()) { 0.0 } else { r }
}

impl Node {
    /// Anything beyond a bare (possibly negated) number or constant.
    fn has_operator(&self) -> bool {
        match self {
            Node::Num(..) | Node::Const(..) => false,
            Node::Neg(n) => n.has_operator(),
            _ => true,
        }
    }

    fn eval(&self) -> Result<f64, CalcError> {
        let v = match self {
            Node::Num(v, _) | Node::Const(_, v) => *v,
            Node::Neg(n) => -n.eval()?,
            Node::Percent(n) => n.eval()? / 100.0,
            Node::Of(p, n) => p.eval()? * n.eval()?,
            Node::Bin(op, l, r) => {
                let a = l.eval()?;
                // `200 + 10%` is 220, like on a pocket calculator.
                let b = match (op, r.as_ref()) {
                    (Bin::Add | Bin::Sub, Node::Percent(p)) => a * p.eval()? / 100.0,
                    _ => r.eval()?,
                };
                match op {
                    Bin::Add => snap_sum(a + b, a, b),
                    Bin::Sub => snap_sum(a - b, a, b),
                    Bin::Mul => a * b,
                    Bin::Div | Bin::Mod if b == 0.0 => return Err(CalcError::DivisionByZero),
                    Bin::Div => a / b,
                    Bin::Mod => {
                        let m = a % b;
                        if m != 0.0 && (m < 0.0) != (b < 0.0) { m + b } else { m }
                    }
                    Bin::Pow => a.powf(b),
                }
            }
            Node::Fact(n) => {
                let x = n.eval()?;
                if x < 0.0 || x.fract() != 0.0 || x > 170.0 {
                    return Err(CalcError::Undefined);
                }
                (2..=x as u32).fold(1.0, |acc, k| acc * k as f64)
            }
            Node::Func(f, n) => {
                let x = n.eval()?;
                let trig = |r: f64| {
                    if x.abs() >= 1.0 && r.abs() <= 4.0 * f64::EPSILON * x.abs() { 0.0 } else { r }
                };
                match *f {
                    "sqrt" => x.sqrt(),
                    "abs" => x.abs(),
                    "round" => x.round(),
                    "floor" => x.floor(),
                    "ceil" => x.ceil(),
                    "ln" => x.ln(),
                    "log" | "log10" => x.log10(),
                    "log2" => x.log2(),
                    "sin" => trig(x.sin()),
                    "cos" => trig(x.cos()),
                    "tan" => trig(x.tan()),
                    "asin" => x.asin(),
                    "acos" => x.acos(),
                    "atan" => x.atan(),
                    "exp" => x.exp(),
                    _ => unreachable!("unknown function {f}"),
                }
            }
        };
        finite(v)
    }

    /// Binding strength, for putting parentheses back only where needed.
    fn prec(&self) -> u8 {
        match self {
            Node::Bin(Bin::Add | Bin::Sub, ..) => 1,
            Node::Bin(Bin::Mul | Bin::Div | Bin::Mod, ..) | Node::Of(..) => 2,
            Node::Neg(_) => 3,
            Node::Bin(Bin::Pow, ..) => 4,
            Node::Fact(_) | Node::Percent(_) => 5,
            Node::Num(..) | Node::Const(..) | Node::Func(..) => 6,
        }
    }
}

struct At<'a>(&'a Node, u8);

impl fmt::Display for At<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.prec() < self.1 {
            write!(f, "({})", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Num(_, lit) => f.write_str(lit),
            Node::Const(name, _) => f.write_str(name),
            Node::Neg(n) => write!(f, "-{}", At(n, 4)),
            Node::Percent(n) => write!(f, "{}%", At(n, 5)),
            Node::Fact(n) => write!(f, "{}!", At(n, 5)),
            Node::Of(p, n) => write!(f, "{p} of {}", At(n, 3)),
            Node::Func(name, n) => write!(f, "{name}({n})"),
            Node::Bin(Bin::Pow, l, r) => write!(f, "{}^{}", At(l, 5), At(r, 3)),
            Node::Bin(op, l, r) => {
                let (sym, p) = match op {
                    Bin::Add => ("+", 1),
                    Bin::Sub => ("-", 1),
                    Bin::Mul => ("*", 2),
                    Bin::Div => ("/", 2),
                    Bin::Mod => ("%", 2),
                    Bin::Pow => unreachable!(),
                };
                write!(f, "{} {sym} {}", At(l, p), At(r, p + 1))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Formatting

fn trim_fraction(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// `v` to [`SIG_DIGITS`] significant digits, integers without `.0`, and
/// scientific notation (`1.5e20`, `2.5e-7`) beyond 1e15 or below 1e-6.
pub fn format_raw(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return v.to_string();
    }
    let a = v.abs();
    if !(1e-6..1e15).contains(&a) {
        let s = format!("{:.*e}", (SIG_DIGITS - 1) as usize, v);
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        return format!("{}e{e}", trim_fraction(m));
    }
    let mag = a.log10().floor() as i32;
    let decimals = (SIG_DIGITS - 1 - mag).clamp(0, 20) as usize;
    let s = format!("{v:.decimals$}");
    let s = trim_fraction(&s);
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// `1234567.5` → `1,234,567.5`; scientific notation is left alone.
pub fn group_thousands(raw: &str) -> String {
    if raw.contains('e') {
        return raw.to_string();
    }
    let (sign, rest) = match raw.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", raw),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut out = String::from(sign);
    for (k, c) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (input, raw answer)
    const ANSWERS: &[(&str, &str)] = &[
        ("2+2", "4"),
        ("2+2*3", "8"),
        ("(2+2)*3", "12"),
        ("(1+2)^10/7", "8435.5714285714"),
        ("0.1+0.2", "0.3"),
        ("0.3-0.1-0.2", "0"),
        ("1/3", "0.33333333333333"),
        ("2/3", "0.66666666666667"),
        ("10/4", "2.5"),
        ("7 % 3", "1"),
        ("-7 % 3", "2"),
        ("10%3", "1"),
        ("50%", "0.5"),
        ("15% of 80", "12"),
        ("15 % of 80", "12"),
        ("200 + 10%", "220"),
        ("200 - 10%", "180"),
        ("200 * 10%", "20"),
        ("50% + 1", "1.5"),
        ("2^10", "1024"),
        ("2**10", "1024"),
        ("2^3^2", "512"),
        ("-2^2", "-4"),
        ("(-2)^2", "4"),
        ("2^-1", "0.5"),
        ("-3+5", "2"),
        ("--3+1", "4"),
        ("+3-1", "2"),
        ("6×7", "42"),
        ("84÷2", "42"),
        ("3 x 4", "12"),
        ("10 − 4", "6"),
        ("1e3+1", "1001"),
        ("2.5e-3*2", "0.005"),
        (".5+.25", "0.75"),
        ("1,234,567+1", "1234568"),
        ("1_000_000/4", "250000"),
        ("sqrt(2)", "1.4142135623731"),
        ("sqrt(16)+1", "5"),
        ("√16", "4"),
        ("abs(-3.5)", "3.5"),
        ("round(2.5)", "3"),
        ("floor(-2.5)", "-3"),
        ("ceil(2.1)", "3"),
        ("ln(e)", "1"),
        ("log(1000)", "3"),
        ("log10(100)", "2"),
        ("log2(1024)", "10"),
        ("exp(0)", "1"),
        ("sin(pi)", "0"),
        ("cos(pi)", "-1"),
        ("sin(pi/2)", "1"),
        ("tan(0)", "0"),
        ("atan(1)*4", "3.1415926535898"),
        ("asin(1)", "1.5707963267949"),
        ("acos(1)", "0"),
        ("SQRT(4)", "2"),
        ("2*pi", "6.2831853071796"),
        ("π*2", "6.2831853071796"),
        ("tau/2", "3.1415926535898"),
        ("e^2", "7.3890560989306"),
        ("5!", "120"),
        ("3!+1", "7"),
        ("(2+1)!", "6"),
        ("0!", "1"),
        ("170!", "7.257415615308e306"),
        ("2^50", "1.1258999068426e15"),
        ("999999999*1000", "999999999000"),
        ("1/1000000000", "1e-9"),
        ("1/1000", "0.001"),
        ("1e300*1e5", "1e305"),
        ("  2 + 2  ", "4"),
        ("=2+2", "4"),
        ("2+2=", "4"),
        ("2024 - 1 - 5", "2018"),
        ("1000-250", "750"),
        ("12 / 25 / 2024", "0.00023715415019763"),
        ("0 - 5", "-5"),
        ("2000-1000-500", "500"),
        ("5000/10/5", "100"),
        ("100/4/5", "5"),
        ("10-2-3-1", "4"),
        ("1500-1000", "500"),
        ("2024-2023", "1"),
        ("45-13-2024", "-1992"),
        ("12/25/7", "0.068571428571429"),
        ("2000/10/5", "40"),
        ("2024-01-05", "2018"),
        ("2024-1-5", "2018"),
        ("2024/01/05", "404.8"),
        ("12/25/2024", "0.00023715415019763"),
        ("555-1234", "-679"),
        ("555-123-4567", "-4135"),
        ("+1-555-123-4567", "-5244"),
        ("2023-2024", "-1"),
        ("01+1", "2"),
        ("007*2", "14"),
        ("5(6+7)", "65"),
        ("(1+2)(3+4)", "21"),
        ("2(3)", "6"),
        ("2pi", "6.2831853071796"),
        ("2√9", "6"),
        ("3sqrt(16)", "12"),
        ("(2+3)pi", "15.707963267949"),
        ("-2(3)", "-6"),
        ("2^2(3)", "12"),
        ("5!(2)", "240"),
        ("6/2(1+2)", "9"),
    ];

    /// Queries that must not produce an answer.
    const NOT_EXPRESSIONS: &[&str] = &[
        "",
        "   ",
        "2024",
        "3.14",
        "-5",
        "1e3",
        "1,234",
        "pi",
        "e",
        "(5)",
        "-(5)",
        "05.01.2024",
        "10:30",
        "IMG_1234",
        "IMG_1234.jpg",
        "v2",
        "v2+1",
        "1.2.3",
        "1.2.3+1",
        "report-2",
        "report-2.pdf",
        "(555) 123-4567",
        "+1 555 123 4567",
        "1,23+1",
        "1,2345+1",
        "1,000_000+1",
        "2+",
        "*2",
        "2+2)",
        "(2+2",
        "2 2",
        "2+2 abc",
        "abc 2+2",
        "sqrt",
        "sqrt 2",
        "sqrt(x)",
        "sin(2",
        "1920x1080",
        "x2",
        "3x4",
        "mp3",
        "h264",
        "5.+1",
        "hello!",
        "what is 2+2",
        "3 of 4",
        "2+2 = 4",
        "app: 2+2",
        "cv in:Downloads",
        "#1+1",
        "1$+1",
    ];

    #[test]
    fn answers() {
        for (input, want) in ANSWERS {
            match evaluate(input) {
                Ok(c) => assert_eq!(c.raw, *want, "{input:?} (expr {:?})", c.expr),
                Err(e) => panic!("{input:?}: {e}"),
            }
        }
    }

    #[test]
    fn not_expressions() {
        for input in NOT_EXPRESSIONS {
            let r = evaluate(input);
            assert_eq!(r, Err(CalcError::NotAnExpression), "{input:?} → {r:?}");
        }
    }

    #[test]
    fn errors() {
        let cases = [
            ("1/0", CalcError::DivisionByZero),
            ("5 % 0", CalcError::DivisionByZero),
            ("1/(2-2)", CalcError::DivisionByZero),
            ("0/0", CalcError::DivisionByZero),
            ("sqrt(-1)", CalcError::Undefined),
            ("ln(0)", CalcError::Undefined),
            ("log(-1)", CalcError::Undefined),
            ("asin(2)", CalcError::Undefined),
            ("2^5000", CalcError::Undefined),
            ("1e308*10", CalcError::Undefined),
            ("0^-1", CalcError::Undefined),
            ("(-8)^(1/3)", CalcError::Undefined),
            ("2.5!", CalcError::Undefined),
            ("(-1)!", CalcError::Undefined),
            ("171!", CalcError::Undefined),
        ];
        for (input, want) in cases {
            assert_eq!(evaluate(input), Err(want), "{input:?}");
        }
    }

    #[test]
    fn normalized_expression() {
        let cases = [
            ("2+2*3", "2 + 2 * 3"),
            ("(1+2)^10/7", "(1 + 2)^10 / 7"),
            ("((2))+((3))", "2 + 3"),
            ("1-(2-3)", "1 - (2 - 3)"),
            ("(1-2)-3", "1 - 2 - 3"),
            ("2*(3/4)", "2 * (3 / 4)"),
            ("-2^2", "-2^2"),
            ("(-2)^2", "(-2)^2"),
            ("2^3^2", "2^3^2"),
            ("(2^3)^2", "(2^3)^2"),
            ("2^-1", "2^-1"),
            ("--3+1", "-(-3) + 1"),
            ("6×7", "6 * 7"),
            ("84÷2", "84 / 2"),
            ("3 x 4", "3 * 4"),
            ("2**3", "2^3"),
            ("1,234,567+1", "1234567 + 1"),
            (".5+1", "0.5 + 1"),
            ("1e3+1", "1e3 + 1"),
            ("SQRT(2)", "sqrt(2)"),
            ("√(2+2)", "sqrt(2 + 2)"),
            ("15% of 80", "15% of 80"),
            ("200+10%", "200 + 10%"),
            ("(2+1)!", "(2 + 1)!"),
            ("10 % 3", "10 % 3"),
            ("2*pi", "2 * pi"),
            ("5(6+7)", "5 * (6 + 7)"),
            ("(1+2)(3+4)", "(1 + 2) * (3 + 4)"),
            ("2pi", "2 * pi"),
            ("6/2(1+2)", "6 / 2 * (1 + 2)"),
        ];
        for (input, want) in cases {
            assert_eq!(evaluate(input).map(|c| c.expr), Ok(want.to_string()), "{input:?}");
        }
    }

    #[test]
    fn formatting() {
        let cases = [
            (0.0, "0", "0"),
            (-0.0, "0", "0"),
            (4.0, "4", "4"),
            (-1234.5, "-1234.5", "-1,234.5"),
            (1234567.0, "1234567", "1,234,567"),
            (123.0, "123", "123"),
            (999_999_999_999_999.0, "999999999999999", "999,999,999,999,999"),
            (1e15, "1e15", "1e15"),
            (1.5e20, "1.5e20", "1.5e20"),
            (-2.5e-7, "-2.5e-7", "-2.5e-7"),
            (0.000001, "0.000001", "0.000001"),
            (0.1 + 0.2, "0.3", "0.3"),
            (1.1 * 1.1, "1.21", "1.21"),
            (8435.571428571428, "8435.5714285714", "8,435.5714285714"),
        ];
        for (v, raw, display) in cases {
            let r = format_raw(v);
            assert_eq!(r, raw, "{v}");
            assert_eq!(group_thousands(&r), display, "{v}");
        }
    }

    #[test]
    fn calculation_fields() {
        let c = evaluate("1000*1234.5").unwrap();
        assert_eq!(c.input, "1000*1234.5");
        assert_eq!(c.expr, "1000 * 1234.5");
        assert_eq!(c.value, 1234500.0);
        assert_eq!(c.raw, "1234500");
        assert_eq!(c.display, "1,234,500");
        // The value is the rounded answer, not the float noise.
        assert_eq!(evaluate("0.1+0.2").unwrap().value, 0.3);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["display"], "1,234,500");
        assert_eq!(json["value"], 1234500.0);
    }
}
