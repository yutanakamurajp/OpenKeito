//! Mathematical expression control block: parses and compiles scalar
//! expressions of the input signals into bytecode for fast evaluation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{input_list, ControlSpec, Scope};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::diagnostic::Diagnostic;

const MAX_PARSE_DEPTH: usize = 256;

/// The format version that added the comparison operators (`<`, `<=`, `>`,
/// `>=`, `==`, `!=`), the logical operators (`!`, `&&`, `||`) and the
/// functions of [`FUNCTIONS_ADDED_IN_0_10`] to `expression`.
pub const EXPRESSION_LOGIC_SINCE: &str = "0.10";

/// The functions of `expression` since format 0.8.
const FUNCTIONS_SINCE_0_8: &[&str] = &[
    "sin", "cos", "tan", "atan", "atan2", "abs", "sqrt", "exp", "ln", "sign", "min", "max",
    "hypot", "pow",
];

/// The functions that format 0.10 adds. A model of an older version may use
/// these names for signals and variables.
const FUNCTIONS_ADDED_IN_0_10: &[&str] = &["acos", "asin", "floor", "ceil", "round", "trunc"];

/// Scalar mathematical expression of the input signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub name: String,
    /// Input signal names. Signals that are valid identifiers `[A-Za-z_][A-Za-z0-9_]*`
    /// and not built-in function names can be referenced directly; any input can
    /// also be referenced positionally as `u0`, `u1`, ... From format "0.10" on,
    /// acos, asin, floor, ceil, round and trunc are function names too (reserved
    /// only in models of "0.10" and later).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// Mapping from expression variable names (`[A-Za-z_][A-Za-z0-9_]*`) to
    /// signal names, for signals containing `.` (such as `v.d`) or aliases.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    /// Mathematical expression string: numbers, variables, `pi`, `e`, the
    /// arithmetic operators `+ - * / ^`, from "0.10" on the comparison and
    /// logical operators `< <= > >= == != ! && ||` (1 for true, 0 for false),
    /// and function calls. Operators, precedence and functions: docs/model.md,
    /// "Controls", "Expression".
    pub expression: String,
}

/// Stack-machine instruction of a [`CompiledExpression`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExprOp {
    PushConst(f64),
    PushArg(usize),
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Sin,
    Cos,
    Tan,
    Atan,
    Atan2,
    Abs,
    Sqrt,
    Exp,
    Ln,
    Sign,
    Hypot,
    Min(usize),
    Max(usize),
    Acos,
    Asin,
    Floor,
    Ceil,
    Round,
    Trunc,
    /// Logical not: 1 for 0 (or -0), 0 for any other number, NaN for NaN.
    Not,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// Syntax of `expression` that a format version after 0.8 added, as used in
/// one expression: what (an operator such as `'>='` or a function name) and
/// the 1-based character column where it is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprAddition {
    /// The format version that added it.
    pub version: &'static str,
    /// The operator in quotes (`'&&'`) or the function name (`floor`).
    pub what: String,
    /// 1-based character column in `expression`.
    pub column: usize,
}

/// Compiled bytecode representation of an [`Expression`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledExpression {
    ops: Vec<ExprOp>,
    resolved_signals: Vec<String>,
    max_depth: usize,
    additions: Vec<ExprAddition>,
}

/// 1 for true and +0 for false: the value of a comparison or a logical
/// operator.
#[inline]
fn truth(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// A comparison of two numbers; NaN when either is NaN.
#[inline]
fn compare(a: f64, b: f64, f: impl Fn(f64, f64) -> bool) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        truth(f(a, b))
    }
}

/// A logical operator on two numbers, each true unless 0 or -0; NaN when
/// either is NaN. Both are always evaluated (no short circuit).
#[inline]
fn logic(a: f64, b: f64, f: impl Fn(bool, bool) -> bool) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        truth(f(a != 0.0, b != 0.0))
    }
}

impl CompiledExpression {
    /// Uses of syntax added by a format version after 0.8, in the order the
    /// parser meets them: [`Expression::compile_for`] accepts them for any
    /// version, and validation reports E102 when the model declares an older
    /// one.
    pub fn additions(&self) -> &[ExprAddition] {
        &self.additions
    }

    /// Signal names required at evaluation time, indexed to match `args` in
    /// [`CompiledExpression::eval`].
    pub fn resolved_signals(&self) -> &[String] {
        &self.resolved_signals
    }

    /// Compiled bytecode instructions in evaluation order.
    pub fn ops(&self) -> &[ExprOp] {
        &self.ops
    }

    /// Evaluates the expression with signal values corresponding to
    /// [`CompiledExpression::resolved_signals`].
    pub fn eval(&self, args: &[f64]) -> f64 {
        let mut buf = [0.0_f64; 64];
        let mut heap = Vec::new();
        let stack: &mut [f64] = if self.max_depth <= buf.len() {
            &mut buf
        } else {
            heap.resize(self.max_depth, 0.0);
            &mut heap
        };
        let mut sp = 0usize;

        for &op in &self.ops {
            match op {
                ExprOp::PushConst(v) => {
                    stack[sp] = v;
                    sp += 1;
                }
                ExprOp::PushArg(idx) => {
                    stack[sp] = args.get(idx).copied().unwrap_or(f64::NAN);
                    sp += 1;
                }
                ExprOp::Neg => {
                    stack[sp - 1] = -stack[sp - 1];
                }
                ExprOp::Add => {
                    let rhs = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] += rhs;
                }
                ExprOp::Sub => {
                    let rhs = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] -= rhs;
                }
                ExprOp::Mul => {
                    let rhs = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] *= rhs;
                }
                ExprOp::Div => {
                    let rhs = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] /= rhs;
                }
                ExprOp::Pow => {
                    let exp = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].powf(exp);
                }
                ExprOp::Sin => stack[sp - 1] = stack[sp - 1].sin(),
                ExprOp::Cos => stack[sp - 1] = stack[sp - 1].cos(),
                ExprOp::Tan => stack[sp - 1] = stack[sp - 1].tan(),
                ExprOp::Atan => stack[sp - 1] = stack[sp - 1].atan(),
                ExprOp::Atan2 => {
                    let x = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].atan2(x);
                }
                ExprOp::Abs => stack[sp - 1] = stack[sp - 1].abs(),
                ExprOp::Sqrt => stack[sp - 1] = stack[sp - 1].sqrt(),
                ExprOp::Exp => stack[sp - 1] = stack[sp - 1].exp(),
                ExprOp::Ln => stack[sp - 1] = stack[sp - 1].ln(),
                ExprOp::Sign => {
                    let x = stack[sp - 1];
                    stack[sp - 1] = if x > 0.0 {
                        1.0
                    } else if x < 0.0 {
                        -1.0
                    } else {
                        x
                    };
                }
                ExprOp::Hypot => {
                    let y = stack[sp - 1];
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].hypot(y);
                }
                ExprOp::Min(n) => {
                    let start = sp - n;
                    let mut acc = stack[start];
                    for &v in &stack[start + 1..sp] {
                        if v < acc || v.is_nan() {
                            acc = v;
                        }
                    }
                    sp = start + 1;
                    stack[start] = acc;
                }
                ExprOp::Max(n) => {
                    let start = sp - n;
                    let mut acc = stack[start];
                    for &v in &stack[start + 1..sp] {
                        if v > acc || v.is_nan() {
                            acc = v;
                        }
                    }
                    sp = start + 1;
                    stack[start] = acc;
                }
                ExprOp::Acos => stack[sp - 1] = stack[sp - 1].acos(),
                ExprOp::Asin => stack[sp - 1] = stack[sp - 1].asin(),
                ExprOp::Floor => stack[sp - 1] = stack[sp - 1].floor(),
                ExprOp::Ceil => stack[sp - 1] = stack[sp - 1].ceil(),
                // Halves away from 0, like C's round.
                ExprOp::Round => stack[sp - 1] = stack[sp - 1].round(),
                ExprOp::Trunc => stack[sp - 1] = stack[sp - 1].trunc(),
                ExprOp::Not => {
                    let x = stack[sp - 1];
                    stack[sp - 1] = if x.is_nan() { x } else { truth(x == 0.0) };
                }
                ExprOp::Lt
                | ExprOp::Le
                | ExprOp::Gt
                | ExprOp::Ge
                | ExprOp::Eq
                | ExprOp::Ne
                | ExprOp::And
                | ExprOp::Or => {
                    let b = stack[sp - 1];
                    sp -= 1;
                    let a = stack[sp - 1];
                    stack[sp - 1] = match op {
                        ExprOp::Lt => compare(a, b, |a, b| a < b),
                        ExprOp::Le => compare(a, b, |a, b| a <= b),
                        ExprOp::Gt => compare(a, b, |a, b| a > b),
                        ExprOp::Ge => compare(a, b, |a, b| a >= b),
                        ExprOp::Eq => compare(a, b, |a, b| a == b),
                        ExprOp::Ne => compare(a, b, |a, b| a != b),
                        ExprOp::And => logic(a, b, |a, b| a && b),
                        _ => logic(a, b, |a, b| a || b),
                    };
                }
            }
        }
        debug_assert_eq!(sp, 1);
        stack[0]
    }
}

/// Where in an [`Expression`] block a compile error occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprErrorLocation {
    /// Inside `expression`, at 1-based character column `column`.
    Expression { column: usize },
    /// In `variables.<key>`.
    Variable(String),
    /// In `inputs[index]`.
    Input(usize),
}

/// Compilation error returned by [`Expression::compile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub location: ExprErrorLocation,
    pub message: String,
    pub hint: String,
}

impl ExprError {
    /// Sub-path relative to the control block, e.g. `expression` or
    /// `variables.vd`.
    pub fn field_path(&self) -> String {
        match &self.location {
            ExprErrorLocation::Expression { .. } => "expression".to_string(),
            ExprErrorLocation::Variable(key) => {
                if key.is_empty() {
                    "variables".to_string()
                } else {
                    format!("variables.{key}")
                }
            }
            ExprErrorLocation::Input(idx) => format!("inputs[{idx}]"),
        }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ExprError {}

impl Expression {
    /// Validates bindings and parses `self.expression` into a
    /// [`CompiledExpression`], for a model of the current format version.
    pub fn compile(&self) -> Result<CompiledExpression, ExprError> {
        self.compile_for(crate::VERSION)
    }

    /// Like [`Expression::compile`], for a model that declares the format
    /// version `version`.
    ///
    /// The names of the functions that a version adds are reserved only from
    /// that version on: in an older model they stay free for signals and
    /// variables. Operators and functions of a newer version are compiled
    /// all the same and listed in [`CompiledExpression::additions`] (for
    /// E102), unless the name of such a function is a signal or variable of
    /// the block (then the call is an unknown function, as before).
    pub fn compile_for(&self, version: &str) -> Result<CompiledExpression, ExprError> {
        let names = Names {
            logic: crate::version_at_least(version, EXPRESSION_LOGIC_SINCE),
        };
        let mut seen_inputs: HashSet<&str> = HashSet::with_capacity(self.inputs.len());
        let mut input_idents: HashMap<&str, usize> = HashMap::with_capacity(self.inputs.len());
        for (i, sig) in self.inputs.iter().enumerate() {
            if !seen_inputs.insert(sig.as_str()) {
                return Err(ExprError {
                    location: ExprErrorLocation::Input(i),
                    message: format!("duplicate input signal \"{sig}\" at inputs[{i}]"),
                    hint: "List each input signal only once in \"inputs\".".to_string(),
                });
            }
            if is_valid_ident(sig) && !names.is_builtin_fn(sig) {
                input_idents.insert(sig.as_str(), i);
            }
        }

        for var_name in self.variables.keys() {
            if !is_valid_ident(var_name) {
                return Err(ExprError {
                    location: ExprErrorLocation::Variable(var_name.clone()),
                    message: format!("variable name \"{var_name}\" is not a valid identifier"),
                    hint: "Use letters, digits and '_' only, starting with a letter or '_' \
                           ([A-Za-z_][A-Za-z0-9_]*)."
                        .to_string(),
                });
            }
            if names.is_builtin_fn(var_name) {
                return Err(ExprError {
                    location: ExprErrorLocation::Variable(var_name.clone()),
                    message: format!(
                        "variable name \"{var_name}\" collides with the built-in function \"{var_name}\""
                    ),
                    hint: format!(
                        "Choose a different variable name in \"variables\" instead of \"{var_name}\"."
                    ),
                });
            }
            if input_idents.contains_key(var_name.as_str()) {
                return Err(ExprError {
                    location: ExprErrorLocation::Variable(var_name.clone()),
                    message: format!(
                        "variable name \"{var_name}\" collides with the signal \"{var_name}\" in \"inputs\""
                    ),
                    hint: format!(
                        "Remove \"{var_name}\" from \"variables\", or use a different variable name."
                    ),
                });
            }
        }

        let tokens = tokenize(&self.expression)?;
        let mut parser = Parser {
            src: &self.expression,
            tokens: &tokens,
            pos: 0,
            expr: self,
            names,
            input_idents: &input_idents,
            signal_slots: HashMap::new(),
            resolved_signals: Vec::new(),
            ops: Vec::new(),
            additions: Vec::new(),
        };

        if parser.peek().kind == TokenKind::Eof {
            return Err(ExprError {
                location: ExprErrorLocation::Expression { column: 1 },
                message: "expression is empty".to_string(),
                hint: "Write a mathematical expression, for example \"2*x^2 + sin(x)\"."
                    .to_string(),
            });
        }

        parser.parse_expr(0)?;

        let trailing = parser.peek();
        if trailing.kind != TokenKind::Eof {
            return Err(parser.unexpected_trailing_error(trailing));
        }

        let max_depth = compute_max_depth(&parser.ops);
        Ok(CompiledExpression {
            ops: parser.ops,
            resolved_signals: parser.resolved_signals,
            max_depth,
            additions: parser.additions,
        })
    }
}

impl ControlSpec for Expression {
    fn name(&self) -> &str {
        &self.name
    }

    fn type_name(&self) -> &'static str {
        "expression"
    }

    fn since(&self) -> &'static str {
        "0.8"
    }

    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = input_list(&self.inputs);
        for (k, sig) in &self.variables {
            v.push((format!("variables.{k}"), sig.as_str()));
        }
        v
    }

    fn is_direct(&self, _field: &str, _time_step_s: f64) -> bool {
        true
    }

    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        if let Err(err) = self.compile_for(scope.version) {
            d.push(Diagnostic::error(
                "E243",
                format!("{p}.{}", err.field_path()),
                format!("expression of \"{}\": {}", self.name, err.message),
                err.hint,
            ));
        }
    }

    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "expression",
            category: "control",
            summary: "Scalar mathematical expression of the input signals. Supports +, -, *, /, ^ \
                      (right-associative, higher precedence than unary +/-), parentheses, \
                      constants pi and e, and functions sin, cos, tan, atan, atan2, abs, sqrt, \
                      exp, ln, sign, min, max, hypot, pow; from version 0.10 also the \
                      comparisons <, <=, >, >=, ==, != and the logical operators !, &&, || \
                      (precedence as in C; true is 1 and false 0; a NaN operand gives NaN; \
                      both sides of && and || are evaluated; a < b < c is rejected) and the \
                      functions acos, asin, floor, ceil, round (halves away from 0) and trunc. \
                      Angles are in radians. A non-finite result stops the run (E251).",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p(
                    "inputs",
                    "",
                    false,
                    "Names of the input signals. Signals that are valid identifiers (and not \
                     built-in function names) can be used directly; any input can be referenced \
                     positionally as u0, u1, ....",
                ),
                p(
                    "variables",
                    "",
                    false,
                    "Map from expression variable names ([A-Za-z_][A-Za-z0-9_]*) to signal \
                     names, for signals containing '.' (such as v.d) or aliases.",
                ),
                p("expression", "", true, "Mathematical expression string."),
            ]),
            example: json!({"type": "expression", "name": "expr_out", "inputs": ["x"], "expression": "2*x^2 + sin(x)"}),
        }
    }
}

fn is_valid_ident(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The built-in function names of a format version.
#[derive(Debug, Clone, Copy)]
struct Names {
    /// The version has the comparison and logical operators and the
    /// functions of [`FUNCTIONS_ADDED_IN_0_10`].
    logic: bool,
}

impl Names {
    fn is_builtin_fn(self, s: &str) -> bool {
        FUNCTIONS_SINCE_0_8.contains(&s) || (self.logic && FUNCTIONS_ADDED_IN_0_10.contains(&s))
    }

    fn list(self) -> String {
        let mut v = FUNCTIONS_SINCE_0_8.to_vec();
        if self.logic {
            v.extend_from_slice(FUNCTIONS_ADDED_IN_0_10);
        }
        v.join(", ")
    }
}

fn parse_positional_input(ident: &str) -> Option<usize> {
    let digits = ident.strip_prefix('u')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    digits.parse().ok()
}

fn char_column(src: &str, byte_pos: usize) -> usize {
    src[..byte_pos].chars().count() + 1
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TokenKind<'a> {
    Number(f64),
    Ident(&'a str),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    Bang,
    Eof,
}

#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    kind: TokenKind<'a>,
    start_byte: usize,
    column: usize,
    adjacent_to_prev: bool,
}

fn dot_hint() -> String {
    "Signal names containing '.' cannot be written directly in an expression; map them in \
     \"variables\" (for example {\"vd\": \"v.d\"}) or refer to inputs by position (u0, u1, ...)."
        .to_string()
}

fn tokenize(src: &str) -> Result<Vec<Token<'_>>, ExprError> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut prev_end = 0usize;

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        let column = char_column(src, start);
        let adjacent_to_prev = !tokens.is_empty() && start == prev_end;
        let b = bytes[i];

        let kind = match b {
            b'+' => {
                i += 1;
                TokenKind::Plus
            }
            b'-' => {
                i += 1;
                TokenKind::Minus
            }
            b'*' => {
                i += 1;
                TokenKind::Star
            }
            b'/' => {
                i += 1;
                TokenKind::Slash
            }
            b'^' => {
                i += 1;
                TokenKind::Caret
            }
            b'(' => {
                i += 1;
                TokenKind::LParen
            }
            b')' => {
                i += 1;
                TokenKind::RParen
            }
            b',' => {
                i += 1;
                TokenKind::Comma
            }
            b'<' | b'>' | b'=' | b'!' | b'&' | b'|' => {
                let next = bytes.get(i + 1).copied();
                let (kind, len) = match (b, next) {
                    (b'<', Some(b'=')) => (TokenKind::Le, 2),
                    (b'<', _) => (TokenKind::Lt, 1),
                    (b'>', Some(b'=')) => (TokenKind::Ge, 2),
                    (b'>', _) => (TokenKind::Gt, 1),
                    (b'=', Some(b'=')) => (TokenKind::EqEq, 2),
                    (b'!', Some(b'=')) => (TokenKind::Ne, 2),
                    (b'!', _) => (TokenKind::Bang, 1),
                    (b'&', Some(b'&')) => (TokenKind::AndAnd, 2),
                    (b'|', Some(b'|')) => (TokenKind::OrOr, 2),
                    _ => {
                        let ch = b as char;
                        return Err(ExprError {
                            location: ExprErrorLocation::Expression { column },
                            message: format!("unexpected character '{ch}' at character {column}"),
                            hint: "Write '==' to compare for equality, '&&' for logical and \
                                   and '||' for logical or: a single '=', '&' or '|' is not an \
                                   operator."
                                .to_string(),
                        });
                    }
                };
                i += len;
                kind
            }
            b'.' => {
                if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    let (val, next_i) = lex_number(src, start)?;
                    i = next_i;
                    TokenKind::Number(val)
                } else {
                    return Err(ExprError {
                        location: ExprErrorLocation::Expression { column },
                        message: format!("unexpected '.' at character {column}"),
                        hint: dot_hint(),
                    });
                }
            }
            b'0'..=b'9' => {
                let (val, next_i) = lex_number(src, start)?;
                i = next_i;
                TokenKind::Number(val)
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                TokenKind::Ident(&src[start..i])
            }
            _ => {
                let ch = src[start..].chars().next().unwrap_or('?');
                return Err(ExprError {
                    location: ExprErrorLocation::Expression { column },
                    message: format!("unexpected character '{ch}' at character {column}"),
                    hint: "Expressions allow numbers, identifiers, parentheses, commas, the \
                           operators +, -, *, /, ^, the comparisons <, <=, >, >=, ==, != and the \
                           logical operators !, &&, ||."
                        .to_string(),
                });
            }
        };

        prev_end = i;
        tokens.push(Token {
            kind,
            start_byte: start,
            column,
            adjacent_to_prev,
        });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        start_byte: src.len(),
        column: char_column(src, src.len()),
        adjacent_to_prev: false,
    });
    Ok(tokens)
}

fn lex_number(src: &str, start: usize) -> Result<(f64, usize), ExprError> {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let exp_col = char_column(src, i);
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == digit_start {
            return Err(ExprError {
                location: ExprErrorLocation::Expression { column: exp_col },
                message: format!("incomplete exponent in number literal at character {exp_col}"),
                hint: "Write exponent digits after 'e' or 'E' (for example 1e-3 or 2.5E+4), or \
                       write '*' for multiplication (for example 2*e)."
                    .to_string(),
            });
        }
    }
    let slice = &src[start..i];
    let col = char_column(src, start);
    let val: f64 = slice.parse().map_err(|_| ExprError {
        location: ExprErrorLocation::Expression { column: col },
        message: format!("invalid number literal \"{slice}\" at character {col}"),
        hint: "Write a valid finite decimal or scientific number literal.".to_string(),
    })?;
    if !val.is_finite() {
        return Err(ExprError {
            location: ExprErrorLocation::Expression { column: col },
            message: format!("number literal \"{slice}\" at character {col} is not finite"),
            hint: "Use a finite IEEE 754 floating-point number (within about +/-1.8e308)."
                .to_string(),
        });
    }
    Ok((val, i))
}

struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Token<'a>],
    pos: usize,
    expr: &'a Expression,
    names: Names,
    input_idents: &'a HashMap<&'a str, usize>,
    signal_slots: HashMap<String, usize>,
    resolved_signals: Vec<String>,
    ops: Vec<ExprOp>,
    additions: Vec<ExprAddition>,
}

/// The relational operator (`<`, `<=`, `>`, `>=`) of a token.
fn relational_op(kind: TokenKind<'_>) -> Option<ExprOp> {
    match kind {
        TokenKind::Lt => Some(ExprOp::Lt),
        TokenKind::Le => Some(ExprOp::Le),
        TokenKind::Gt => Some(ExprOp::Gt),
        TokenKind::Ge => Some(ExprOp::Ge),
        _ => None,
    }
}

/// The equality operator (`==`, `!=`) of a token.
fn equality_op(kind: TokenKind<'_>) -> Option<ExprOp> {
    match kind {
        TokenKind::EqEq => Some(ExprOp::Eq),
        TokenKind::Ne => Some(ExprOp::Ne),
        _ => None,
    }
}

/// E243 for a comparison whose operand is another comparison without
/// parentheses: `second` is the later of the two operators.
fn chained_comparison(first: Token<'_>, second: Token<'_>) -> ExprError {
    ExprError {
        location: ExprErrorLocation::Expression {
            column: second.column,
        },
        message: format!(
            "comparison {} at character {} is chained with the comparison {} at character {}",
            describe_token(second.kind),
            second.column,
            describe_token(first.kind),
            first.column
        ),
        hint: "Comparisons cannot be chained: write (a < b) && (b < c) to test both, and put a \
               comparison in parentheses to compare its result, for example (a < b) == c."
            .to_string(),
    }
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Token<'a> {
        self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token<'a> {
        let tok = self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check_depth(&self, depth: usize, column: usize) -> Result<(), ExprError> {
        if depth > MAX_PARSE_DEPTH {
            return Err(ExprError {
                location: ExprErrorLocation::Expression { column },
                message: format!(
                    "expression nesting exceeds {MAX_PARSE_DEPTH} levels at character {column}"
                ),
                hint: "Simplify the expression or split it across multiple expression blocks."
                    .to_string(),
            });
        }
        Ok(())
    }

    fn slot_for(&mut self, signal: &str) -> usize {
        if let Some(&idx) = self.signal_slots.get(signal) {
            return idx;
        }
        let idx = self.resolved_signals.len();
        self.resolved_signals.push(signal.to_string());
        self.signal_slots.insert(signal.to_string(), idx);
        idx
    }

    /// Notes the use of an operator or function that format 0.10 added.
    fn added(&mut self, what: String, column: usize) {
        self.additions.push(ExprAddition {
            version: EXPRESSION_LOGIC_SINCE,
            what,
            column,
        });
    }

    fn parse_expr(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_or(depth)
    }

    fn parse_or(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_and(depth)?;
        while self.peek().kind == TokenKind::OrOr {
            let tok = self.advance();
            self.added(describe_token(tok.kind), tok.column);
            self.parse_and(depth)?;
            self.ops.push(ExprOp::Or);
        }
        Ok(())
    }

    fn parse_and(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_equality(depth)?;
        while self.peek().kind == TokenKind::AndAnd {
            let tok = self.advance();
            self.added(describe_token(tok.kind), tok.column);
            self.parse_equality(depth)?;
            self.ops.push(ExprOp::And);
        }
        Ok(())
    }

    /// `==` and `!=`. An operand that is itself a comparison without
    /// parentheses is E243.
    fn parse_equality(&mut self, depth: usize) -> Result<(), ExprError> {
        let mut last = self.parse_relational(depth)?;
        while let Some(op) = equality_op(self.peek().kind) {
            let tok = self.advance();
            if let Some(first) = last {
                return Err(chained_comparison(first, tok));
            }
            self.added(describe_token(tok.kind), tok.column);
            if let Some(inner) = self.parse_relational(depth)? {
                return Err(chained_comparison(tok, inner));
            }
            self.ops.push(op);
            last = Some(tok);
        }
        Ok(())
    }

    /// `<`, `<=`, `>` and `>=`: the operator token when there is one. A
    /// second relational operator right after the operand is E243.
    fn parse_relational(&mut self, depth: usize) -> Result<Option<Token<'a>>, ExprError> {
        self.parse_add_sub(depth)?;
        let Some(op) = relational_op(self.peek().kind) else {
            return Ok(None);
        };
        let tok = self.advance();
        self.added(describe_token(tok.kind), tok.column);
        self.parse_add_sub(depth)?;
        self.ops.push(op);
        let next = self.peek();
        if relational_op(next.kind).is_some() {
            return Err(chained_comparison(tok, next));
        }
        Ok(Some(tok))
    }

    fn parse_add_sub(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_mul_div(depth)?;
        loop {
            match self.peek().kind {
                TokenKind::Plus => {
                    self.advance();
                    self.parse_mul_div(depth)?;
                    self.ops.push(ExprOp::Add);
                }
                TokenKind::Minus => {
                    self.advance();
                    self.parse_mul_div(depth)?;
                    self.ops.push(ExprOp::Sub);
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn parse_mul_div(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_unary(depth)?;
        loop {
            match self.peek().kind {
                TokenKind::Star => {
                    self.advance();
                    self.parse_unary(depth)?;
                    self.ops.push(ExprOp::Mul);
                }
                TokenKind::Slash => {
                    self.advance();
                    self.parse_unary(depth)?;
                    self.ops.push(ExprOp::Div);
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn parse_unary(&mut self, depth: usize) -> Result<(), ExprError> {
        match self.peek().kind {
            TokenKind::Plus => {
                let tok = self.advance();
                self.check_depth(depth + 1, tok.column)?;
                self.parse_unary(depth + 1)
            }
            TokenKind::Minus => {
                let tok = self.advance();
                self.check_depth(depth + 1, tok.column)?;
                self.parse_unary(depth + 1)?;
                self.ops.push(ExprOp::Neg);
                Ok(())
            }
            TokenKind::Bang => {
                let tok = self.advance();
                self.check_depth(depth + 1, tok.column)?;
                self.added(describe_token(tok.kind), tok.column);
                self.parse_unary(depth + 1)?;
                self.ops.push(ExprOp::Not);
                Ok(())
            }
            _ => self.parse_power(depth),
        }
    }

    fn parse_power(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_primary(depth)?;
        if self.peek().kind == TokenKind::Caret {
            let tok = self.advance();
            self.check_depth(depth + 1, tok.column)?;
            // Right-associative; RHS uses `parse_unary` so `-x^2 == -(x^2)` and `x^-2 == x^(-2)`.
            self.parse_unary(depth + 1)?;
            self.ops.push(ExprOp::Pow);
        }
        Ok(())
    }

    fn parse_primary(&mut self, depth: usize) -> Result<(), ExprError> {
        let tok = self.peek();
        match tok.kind {
            TokenKind::Number(v) => {
                self.advance();
                self.ops.push(ExprOp::PushConst(v));
                Ok(())
            }
            TokenKind::LParen => {
                self.advance();
                self.check_depth(depth + 1, tok.column)?;
                if self.peek().kind == TokenKind::RParen {
                    let close = self.peek();
                    return Err(ExprError {
                        location: ExprErrorLocation::Expression {
                            column: close.column,
                        },
                        message: format!("empty parentheses '()' at character {}", tok.column),
                        hint: "Write an expression inside the parentheses.".to_string(),
                    });
                }
                self.parse_expr(depth + 1)?;
                let close = self.peek();
                if close.kind != TokenKind::RParen {
                    if close.adjacent_to_prev && self.src[close.start_byte..].starts_with('.') {
                        return Err(ExprError {
                            location: ExprErrorLocation::Expression {
                                column: close.column,
                            },
                            message: format!(
                                "unexpected '.' at character {} before closing ')'",
                                close.column
                            ),
                            hint: dot_hint(),
                        });
                    }
                    return Err(ExprError {
                        location: ExprErrorLocation::Expression {
                            column: close.column,
                        },
                        message: format!(
                            "expected ')' at character {} to close '(' from character {}",
                            close.column, tok.column
                        ),
                        hint: "Add the missing ')' or check the operators inside parentheses."
                            .to_string(),
                    });
                }
                self.advance();
                Ok(())
            }
            TokenKind::Ident(name) => {
                self.advance();
                if self.peek().kind == TokenKind::LParen {
                    self.parse_call(name, tok.column, depth)
                } else {
                    self.resolve_ident(name, tok.column)
                }
            }
            TokenKind::Eof => Err(ExprError {
                location: ExprErrorLocation::Expression { column: tok.column },
                message: format!("unexpected end of expression at character {}", tok.column),
                hint: "Complete the expression with a number, variable, or parenthesized subexpression.".to_string(),
            }),
            _ => Err(ExprError {
                location: ExprErrorLocation::Expression { column: tok.column },
                message: format!(
                    "unexpected {} at character {}",
                    describe_token(tok.kind),
                    tok.column
                ),
                hint: "Expected a number, variable, function call, unary +, - or !, or '('."
                    .to_string(),
            }),
        }
    }

    fn parse_call(&mut self, name: &str, name_col: usize, depth: usize) -> Result<(), ExprError> {
        let lparen = self.advance();
        self.check_depth(depth + 1, lparen.column)?;
        let mut argc = 0usize;
        if self.peek().kind == TokenKind::RParen {
            self.advance();
        } else {
            loop {
                self.parse_expr(depth + 1)?;
                argc += 1;
                let next = self.peek();
                match next.kind {
                    TokenKind::Comma => {
                        self.advance();
                    }
                    TokenKind::RParen => {
                        self.advance();
                        break;
                    }
                    _ => {
                        if next.adjacent_to_prev && self.src[next.start_byte..].starts_with('.') {
                            return Err(ExprError {
                                location: ExprErrorLocation::Expression {
                                    column: next.column,
                                },
                                message: format!("unexpected '.' at character {}", next.column),
                                hint: dot_hint(),
                            });
                        }
                        return Err(ExprError {
                            location: ExprErrorLocation::Expression {
                                column: next.column,
                            },
                            message: format!(
                                "expected ',' or ')' in call to \"{name}\" at character {}",
                                next.column
                            ),
                            hint:
                                "Separate function arguments with ',' and close the call with ')'."
                                    .to_string(),
                        });
                    }
                }
            }
        }

        let op = match name {
            "sin" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Sin)?,
            "cos" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Cos)?,
            "tan" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Tan)?,
            "atan" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Atan)?,
            "abs" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Abs)?,
            "sqrt" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Sqrt)?,
            "exp" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Exp)?,
            "ln" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Ln)?,
            "sign" => expect_arity(name, name_col, argc, 1, 1, ExprOp::Sign)?,
            "atan2" => expect_arity(name, name_col, argc, 2, 2, ExprOp::Atan2)?,
            "hypot" => expect_arity(name, name_col, argc, 2, 2, ExprOp::Hypot)?,
            "pow" => expect_arity(name, name_col, argc, 2, 2, ExprOp::Pow)?,
            "min" => expect_arity(name, name_col, argc, 1, usize::MAX, ExprOp::Min(argc))?,
            "max" => expect_arity(name, name_col, argc, 1, usize::MAX, ExprOp::Max(argc))?,
            // In an older model, a function of 0.10 whose name is a signal
            // or variable of the block is an unknown function, as before.
            "acos" | "asin" | "floor" | "ceil" | "round" | "trunc"
                if self.names.logic
                    || !(self.input_idents.contains_key(name)
                        || self.expr.variables.contains_key(name)) =>
            {
                let op = match name {
                    "acos" => ExprOp::Acos,
                    "asin" => ExprOp::Asin,
                    "floor" => ExprOp::Floor,
                    "ceil" => ExprOp::Ceil,
                    "round" => ExprOp::Round,
                    _ => ExprOp::Trunc,
                };
                let op = expect_arity(name, name_col, argc, 1, 1, op)?;
                self.added(name.to_string(), name_col);
                op
            }
            _ => {
                return Err(ExprError {
                    location: ExprErrorLocation::Expression { column: name_col },
                    message: format!("unknown function \"{name}\" at character {name_col}"),
                    hint: format!(
                        "Supported functions: {}. If \"{name}\" is a variable, put an operator \
                         like '*' before '('.",
                        self.names.list()
                    ),
                });
            }
        };
        self.ops.push(op);
        Ok(())
    }

    fn resolve_ident(&mut self, name: &str, column: usize) -> Result<(), ExprError> {
        if self.names.is_builtin_fn(name) {
            return Err(ExprError {
                location: ExprErrorLocation::Expression { column },
                message: format!(
                    "built-in function \"{name}\" used without parentheses at character {column}"
                ),
                hint: format!("Call \"{name}\" with parentheses, for example {name}(...)."),
            });
        }
        if self.input_idents.contains_key(name) {
            let slot = self.slot_for(name);
            self.ops.push(ExprOp::PushArg(slot));
            return Ok(());
        }
        if let Some(sig) = self.expr.variables.get(name) {
            let slot = self.slot_for(sig);
            self.ops.push(ExprOp::PushArg(slot));
            return Ok(());
        }
        if let Some(idx) = parse_positional_input(name) {
            if let Some(sig) = self.expr.inputs.get(idx) {
                let slot = self.slot_for(sig);
                self.ops.push(ExprOp::PushArg(slot));
                return Ok(());
            }
            return Err(ExprError {
                location: ExprErrorLocation::Expression { column },
                message: format!(
                    "positional input \"{name}\" at character {column} is out of range \
                     (\"inputs\" has {} {})",
                    self.expr.inputs.len(),
                    if self.expr.inputs.len() == 1 {
                        "entry"
                    } else {
                        "entries"
                    }
                ),
                hint: if self.expr.inputs.is_empty() {
                    "Add signals to \"inputs\" (u0, u1, ...) or map them in \"variables\"."
                        .to_string()
                } else {
                    format!(
                        "Use u0..u{}, or add another signal to \"inputs\".",
                        self.expr.inputs.len() - 1
                    )
                },
            });
        }
        match name {
            "pi" => {
                self.ops.push(ExprOp::PushConst(std::f64::consts::PI));
                Ok(())
            }
            "e" => {
                self.ops.push(ExprOp::PushConst(std::f64::consts::E));
                Ok(())
            }
            _ => Err(ExprError {
                location: ExprErrorLocation::Expression { column },
                message: format!("unknown variable \"{name}\" at character {column}"),
                hint: self.unknown_variable_hint(name),
            }),
        }
    }

    fn unknown_variable_hint(&self, name: &str) -> String {
        let mut available: Vec<String> = Vec::new();
        for (i, sig) in self.expr.inputs.iter().enumerate() {
            if is_valid_ident(sig) && !self.names.is_builtin_fn(sig) {
                available.push(sig.clone());
            } else {
                available.push(format!("u{i} ({sig})"));
            }
        }
        for k in self.expr.variables.keys() {
            available.push(k.clone());
        }
        if available.is_empty() {
            format!("Add \"{name}\" to \"inputs\" or \"variables\". Built-in constants: pi, e.")
        } else {
            format!(
                "Add \"{name}\" to \"inputs\" or \"variables\". Available variables: {}; \
                 built-in constants: pi, e.",
                available.join(", ")
            )
        }
    }

    fn unexpected_trailing_error(&self, tok: Token<'a>) -> ExprError {
        if tok.adjacent_to_prev && self.src[tok.start_byte..].starts_with('.') {
            return ExprError {
                location: ExprErrorLocation::Expression { column: tok.column },
                message: format!("unexpected '.' at character {}", tok.column),
                hint: dot_hint(),
            };
        }
        let hint = match tok.kind {
            TokenKind::Ident(_) | TokenKind::Number(_) | TokenKind::LParen => {
                "Insert a binary operator such as '+', '-', '*', '/', or '^' between terms."
                    .to_string()
            }
            TokenKind::RParen => {
                "Remove the extra ')' or add a matching '(' earlier in the expression.".to_string()
            }
            TokenKind::Comma => {
                "Commas separate arguments inside a function call such as min(a, b).".to_string()
            }
            _ => "Check the syntax around this position.".to_string(),
        };
        ExprError {
            location: ExprErrorLocation::Expression { column: tok.column },
            message: format!(
                "unexpected {} at character {}",
                describe_token(tok.kind),
                tok.column
            ),
            hint,
        }
    }
}

fn expect_arity(
    name: &str,
    column: usize,
    argc: usize,
    min: usize,
    max: usize,
    op: ExprOp,
) -> Result<ExprOp, ExprError> {
    if argc < min || argc > max {
        let expected = if min == max {
            if min == 1 {
                "1 argument".to_string()
            } else {
                format!("{min} arguments")
            }
        } else {
            format!("at least {min} argument")
        };
        return Err(ExprError {
            location: ExprErrorLocation::Expression { column },
            message: format!(
                "function \"{name}\" at character {column} expects {expected}, got {argc}"
            ),
            hint: format!("Pass {expected} to \"{name}\"."),
        });
    }
    Ok(op)
}

fn describe_token(kind: TokenKind<'_>) -> String {
    match kind {
        TokenKind::Number(v) => format!("number '{v}'"),
        TokenKind::Ident(s) => format!("identifier '{s}'"),
        TokenKind::Plus => "'+'".to_string(),
        TokenKind::Minus => "'-'".to_string(),
        TokenKind::Star => "'*'".to_string(),
        TokenKind::Slash => "'/'".to_string(),
        TokenKind::Caret => "'^'".to_string(),
        TokenKind::LParen => "'('".to_string(),
        TokenKind::RParen => "')'".to_string(),
        TokenKind::Comma => "','".to_string(),
        TokenKind::Lt => "'<'".to_string(),
        TokenKind::Le => "'<='".to_string(),
        TokenKind::Gt => "'>'".to_string(),
        TokenKind::Ge => "'>='".to_string(),
        TokenKind::EqEq => "'=='".to_string(),
        TokenKind::Ne => "'!='".to_string(),
        TokenKind::AndAnd => "'&&'".to_string(),
        TokenKind::OrOr => "'||'".to_string(),
        TokenKind::Bang => "'!'".to_string(),
        TokenKind::Eof => "end of expression".to_string(),
    }
}

fn compute_max_depth(ops: &[ExprOp]) -> usize {
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    for op in ops {
        match op {
            ExprOp::PushConst(_) | ExprOp::PushArg(_) => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            ExprOp::Neg
            | ExprOp::Sin
            | ExprOp::Cos
            | ExprOp::Tan
            | ExprOp::Atan
            | ExprOp::Abs
            | ExprOp::Sqrt
            | ExprOp::Exp
            | ExprOp::Ln
            | ExprOp::Sign
            | ExprOp::Acos
            | ExprOp::Asin
            | ExprOp::Floor
            | ExprOp::Ceil
            | ExprOp::Round
            | ExprOp::Trunc
            | ExprOp::Not => {}
            ExprOp::Add
            | ExprOp::Sub
            | ExprOp::Mul
            | ExprOp::Div
            | ExprOp::Pow
            | ExprOp::Atan2
            | ExprOp::Hypot
            | ExprOp::Lt
            | ExprOp::Le
            | ExprOp::Gt
            | ExprOp::Ge
            | ExprOp::Eq
            | ExprOp::Ne
            | ExprOp::And
            | ExprOp::Or => {
                depth -= 1;
            }
            ExprOp::Min(n) | ExprOp::Max(n) => {
                depth -= n - 1;
            }
        }
    }
    debug_assert_eq!(depth, 1);
    max_depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(inputs: &[&str], variables: &[(&str, &str)], text: &str) -> Expression {
        Expression {
            name: "test".to_string(),
            inputs: inputs.iter().map(|s| (*s).to_string()).collect(),
            variables: variables
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            expression: text.to_string(),
        }
    }

    #[test]
    fn operator_precedence_and_associativity() {
        let c = expr(&["x"], &[], "-x^2").compile().unwrap();
        assert_eq!(c.eval(&[3.0]), -9.0);

        let c = expr(&["x"], &[], "(-x)^2").compile().unwrap();
        assert_eq!(c.eval(&[3.0]), 9.0);

        let c = expr(&["x"], &[], "x^-2").compile().unwrap();
        assert_eq!(c.eval(&[2.0]), 0.25);

        let c = expr(&[], &[], "2^3^2").compile().unwrap();
        assert_eq!(c.eval(&[]), 512.0);

        let c = expr(&[], &[], "1 + 2 * 3 - 4 / 2 + +5").compile().unwrap();
        assert_eq!(c.eval(&[]), 10.0);

        let c = expr(&[], &[], "10 - 3 - 2").compile().unwrap();
        assert_eq!(c.eval(&[]), 5.0);

        let c = expr(&[], &[], "24 / 4 / 2").compile().unwrap();
        assert_eq!(c.eval(&[]), 3.0);
    }

    #[test]
    fn number_literals_support_decimal_and_scientific_notation() {
        let c = expr(&[], &[], "1e-3 + 2.5E+4 + .5 + 2.").compile().unwrap();
        assert!((c.eval(&[]) - 25002.501).abs() < 1e-12);
    }

    #[test]
    fn all_fourteen_functions_evaluate_correctly() {
        let c = expr(&["x", "y"], &[], "sin(x)").compile().unwrap();
        assert!((c.eval(&[1.0]) - 1.0_f64.sin()).abs() < 1e-15);

        let c = expr(&["x"], &[], "cos(x)").compile().unwrap();
        assert!((c.eval(&[1.0]) - 1.0_f64.cos()).abs() < 1e-15);

        let c = expr(&["x"], &[], "tan(x)").compile().unwrap();
        assert!((c.eval(&[0.5]) - 0.5_f64.tan()).abs() < 1e-15);

        let c = expr(&["x"], &[], "atan(x)").compile().unwrap();
        assert!((c.eval(&[1.0]) - std::f64::consts::FRAC_PI_4).abs() < 1e-15);

        let c = expr(&["x"], &[], "abs(x)").compile().unwrap();
        assert_eq!(c.eval(&[-4.5]), 4.5);

        let c = expr(&["x"], &[], "sqrt(x)").compile().unwrap();
        assert_eq!(c.eval(&[9.0]), 3.0);

        let c = expr(&["x"], &[], "exp(x)").compile().unwrap();
        assert!((c.eval(&[1.0]) - std::f64::consts::E).abs() < 1e-15);

        let c = expr(&["x"], &[], "ln(x)").compile().unwrap();
        assert!((c.eval(&[std::f64::consts::E]) - 1.0).abs() < 1e-15);

        let c = expr(&["x"], &[], "sign(x)").compile().unwrap();
        assert_eq!(c.eval(&[2.5]), 1.0);
        assert_eq!(c.eval(&[-2.5]), -1.0);
        assert_eq!(c.eval(&[0.0]), 0.0);
        assert!(c.eval(&[f64::NAN]).is_nan());

        let c = expr(&["y", "x"], &[], "atan2(y, x)").compile().unwrap();
        assert!((c.eval(&[1.0, 0.0]) - std::f64::consts::FRAC_PI_2).abs() < 1e-15);

        let c = expr(&["x", "y"], &[], "hypot(x, y)").compile().unwrap();
        assert_eq!(c.eval(&[3.0, 4.0]), 5.0);

        let c = expr(&["b", "p"], &[], "pow(b, p)").compile().unwrap();
        assert_eq!(c.eval(&[2.0, 5.0]), 32.0);

        let c = expr(&["a", "b", "c"], &[], "min(a, b, c)")
            .compile()
            .unwrap();
        assert_eq!(c.eval(&[3.0, -1.0, 2.0]), -1.0);
        assert!(c.eval(&[f64::NAN, 1.0, 2.0]).is_nan());
        assert!(c.eval(&[1.0, f64::NAN, 2.0]).is_nan());

        let c = expr(&["a", "b", "c"], &[], "max(a, b, c)")
            .compile()
            .unwrap();
        assert_eq!(c.eval(&[3.0, -1.0, 7.0]), 7.0);
        assert!(c.eval(&[f64::NAN, 1.0, 2.0]).is_nan());
        assert!(c.eval(&[1.0, 2.0, f64::NAN]).is_nan());
    }

    #[test]
    fn constants_and_signal_shadowing() {
        let c = expr(&["x"], &[], "pi * x + e^(-x)").compile().unwrap();
        assert_eq!(c.resolved_signals(), ["x"]);
        let expected = std::f64::consts::PI * 2.0 + std::f64::consts::E.powf(-2.0);
        assert!((c.eval(&[2.0]) - expected).abs() < 1e-15);

        // Input named "e" shadows the mathematical constant e.
        let c = expr(&["e"], &[], "2*e").compile().unwrap();
        assert_eq!(c.resolved_signals(), ["e"]);
        assert_eq!(c.eval(&[3.5]), 7.0);

        // Input named "pi" shadows pi.
        let c = expr(&["pi"], &[], "pi + 1").compile().unwrap();
        assert_eq!(c.resolved_signals(), ["pi"]);
        assert_eq!(c.eval(&[10.0]), 11.0);
    }

    #[test]
    fn variables_and_positional_inputs() {
        let c = expr(&[], &[("vd", "v.d"), ("vq", "v.q")], "hypot(vd, vq)")
            .compile()
            .unwrap();
        assert_eq!(c.resolved_signals(), ["v.d", "v.q"]);
        assert_eq!(c.eval(&[3.0, 4.0]), 5.0);

        // Non-identifier input referenced by u0, and identifier input referenced by name.
        let c = expr(&["v.d", "scale"], &[], "u0 * scale + u1")
            .compile()
            .unwrap();
        assert_eq!(c.resolved_signals(), ["v.d", "scale"]);
        assert_eq!(c.eval(&[10.0, 2.0]), 22.0);

        // Signal named after a built-in function cannot be used by name, but works via u0 or variables.
        assert!(expr(&["sin"], &[], "sin").compile().is_err());
        let c = expr(&["sin"], &[], "2 * u0").compile().unwrap();
        assert_eq!(c.resolved_signals(), ["sin"]);
        assert_eq!(c.eval(&[4.0]), 8.0);

        // Explicit inputs name or variables key takes priority over positional u0/u1.
        let c = expr(&["u1", "u0"], &[], "u0 - u1").compile().unwrap();
        assert_eq!(c.resolved_signals(), ["u0", "u1"]);
        assert_eq!(c.eval(&[10.0, 3.0]), 7.0);

        let c = expr(&["v.d"], &[("u0", "override")], "u0")
            .compile()
            .unwrap();
        assert_eq!(c.resolved_signals(), ["override"]);
        assert_eq!(c.eval(&[42.0]), 42.0);
    }

    #[test]
    fn deep_nesting_does_not_overflow_stack_or_eval_buffer() {
        let open = "(".repeat(80);
        let close = ")".repeat(80);
        let text = format!("{open}x + 1{close}");
        let c = expr(&["x"], &[], &text).compile().unwrap();
        assert_eq!(c.eval(&[4.0]), 5.0);

        // Sum of 70 terms requires > 64 stack slots via right-nested parens.
        let mut deep_add = String::new();
        for _ in 0..70 {
            deep_add.push_str("1 + (");
        }
        deep_add.push('1');
        for _ in 0..70 {
            deep_add.push(')');
        }
        let c = expr(&[], &[], &deep_add).compile().unwrap();
        assert_eq!(c.eval(&[]), 71.0);
    }

    #[test]
    fn parse_depth_limit_allows_max_and_rejects_excess() {
        let at_limit = format!(
            "{}x{}",
            "(".repeat(MAX_PARSE_DEPTH),
            ")".repeat(MAX_PARSE_DEPTH)
        );
        let compiled = expr(&["x"], &[], &at_limit).compile().unwrap();
        assert_eq!(compiled.eval(&[3.5]), 3.5);

        let over_limit = format!(
            "{}x{}",
            "(".repeat(MAX_PARSE_DEPTH + 1),
            ")".repeat(MAX_PARSE_DEPTH + 1)
        );
        let err = expr(&["x"], &[], &over_limit).compile().unwrap_err();
        assert_eq!(
            err.location,
            ExprErrorLocation::Expression {
                column: MAX_PARSE_DEPTH + 1
            }
        );
        assert!(
            err.message.contains("exceeds 256 levels"),
            "{}",
            err.message
        );
    }

    #[test]
    fn compile_errors_report_locations_and_hints() {
        let err = expr(&["x"], &[], "").compile().unwrap_err();
        assert_eq!(err.field_path(), "expression");

        let err = expr(&["x"], &[], "   ").compile().unwrap_err();
        assert_eq!(err.field_path(), "expression");

        let err = expr(&[], &[], "1e999").compile().unwrap_err();
        assert_eq!(err.field_path(), "expression");

        let err = expr(&["v.d"], &[], "v.d + 1").compile().unwrap_err();
        assert!(err.hint.contains("\"variables\""), "{}", err.hint);
        assert!(err.hint.contains("u0"), "{}", err.hint);

        let err = expr(&["v.0"], &[], "v.0 + 1").compile().unwrap_err();
        assert!(err.hint.contains("\"variables\""), "{}", err.hint);
        assert!(err.hint.contains("u0"), "{}", err.hint);

        let err = expr(&["x"], &[], "y + 1").compile().unwrap_err();
        assert_eq!(err.location, ExprErrorLocation::Expression { column: 1 });

        let err = expr(&["x"], &[], "u1").compile().unwrap_err();
        assert_eq!(err.location, ExprErrorLocation::Expression { column: 1 });

        let err = expr(&["x"], &[], "foo(x)").compile().unwrap_err();
        assert_eq!(err.location, ExprErrorLocation::Expression { column: 1 });

        for bad in ["sin()", "sin(x, x)", "atan2(x)", "pow(x)", "min()", "max()"] {
            assert!(expr(&["x"], &[], bad).compile().is_err(), "{bad}");
        }

        let err = expr(&[], &[("v.d", "v.d")], "1").compile().unwrap_err();
        assert_eq!(err.field_path(), "variables.v.d");

        let err = expr(&[], &[("sin", "s")], "1").compile().unwrap_err();
        assert_eq!(err.field_path(), "variables.sin");

        let err = expr(&["x"], &[("x", "y")], "x").compile().unwrap_err();
        assert_eq!(err.field_path(), "variables.x");

        let err = expr(&["x", "x"], &[], "x").compile().unwrap_err();
        assert_eq!(err.field_path(), "inputs[1]");
    }

    // Comparison and logical operators, and the functions of 0.10.

    const VARS: &[&str] = &["a", "b", "c", "d", "x"];

    fn ops_of(text: &str) -> Vec<ExprOp> {
        expr(VARS, &[], text)
            .compile()
            .unwrap_or_else(|e| panic!("{text}: {}", e.message))
            .ops()
            .to_vec()
    }

    #[test]
    fn precedence_of_the_comparison_and_logical_operators_is_that_of_c() {
        // Each pair: the text without parentheses, and the same with the
        // parentheses that the rules of C put in (with ^ above the unary
        // operators, as before).
        for (bare, explicit) in [
            ("!x^2", "!(x^2)"),
            ("!x > 0", "(!x) > 0"),
            ("-a < b", "(-a) < b"),
            ("-x^2 < 0", "(-(x^2)) < 0"),
            ("a + b < c", "(a + b) < c"),
            ("a - b >= c * d", "(a - b) >= (c * d)"),
            ("a * b > c / d", "(a * b) > (c / d)"),
            ("a + b == c * d", "(a + b) == (c * d)"),
            ("a != b + c", "a != (b + c)"),
            ("a < b && c <= d", "(a < b) && (c <= d)"),
            ("a == b && c != d", "(a == b) && (c != d)"),
            ("a > b || c >= d", "(a > b) || (c >= d)"),
            ("a && b || c && d", "(a && b) || (c && d)"),
            ("a || b && c", "a || (b && c)"),
            ("a && b && c", "(a && b) && c"),
            ("a || b || c", "(a || b) || c"),
            ("!a && b", "(!a) && b"),
            ("!!a", "!(!a)"),
            ("x^!a", "x^(!a)"),
        ] {
            assert_eq!(ops_of(bare), ops_of(explicit), "{bare} vs {explicit}");
        }
        assert_eq!(
            ops_of("a < b || c"),
            [
                ExprOp::PushArg(0),
                ExprOp::PushArg(1),
                ExprOp::Lt,
                ExprOp::PushArg(2),
                ExprOp::Or
            ]
        );
    }

    #[test]
    fn chained_comparisons_are_rejected_at_the_second_operator() {
        for (text, column) in [
            ("a < b < c", 7),
            ("a >= b > c", 8),
            ("a == b == c", 8),
            ("a != b == c", 8),
            ("a < b == c", 7),
            ("a == b < c", 8),
            ("a <= b != c > d", 8),
            ("(a < b < c) + 1", 8),
        ] {
            let err = expr(VARS, &[], text).compile().unwrap_err();
            assert_eq!(
                err.location,
                ExprErrorLocation::Expression { column },
                "{text}: {}",
                err.message
            );
            assert!(err.message.contains("chained"), "{text}: {}", err.message);
            assert!(
                err.hint.contains("(a < b) && (b < c)"),
                "{text}: {}",
                err.hint
            );
        }
        // With parentheses, or joined by && and ||, they are fine.
        for text in [
            "(a < b) < c",
            "a < (b < c)",
            "(a == b) == c",
            "(a < b) == (c < d)",
            "a < b && b < c",
            "a == b || b == c",
        ] {
            assert!(expr(VARS, &[], text).compile().is_ok(), "{text}");
        }
    }

    #[test]
    fn a_single_equals_ampersand_or_bar_is_rejected() {
        for (text, column) in [
            ("a = b", 3),
            ("a & b", 3),
            ("a | b", 3),
            ("a =< b", 3),
            ("a & & b", 3),
        ] {
            let err = expr(VARS, &[], text).compile().unwrap_err();
            assert_eq!(
                err.location,
                ExprErrorLocation::Expression { column },
                "{text}"
            );
            assert!(err.message.contains("unexpected character"), "{text}");
            assert!(
                err.hint.contains("'=='") && err.hint.contains("'&&'") && err.hint.contains("'||'"),
                "{text}: {}",
                err.hint
            );
        }
        // An operator without an operand.
        for text in ["a <", "&& b", "a || ", "!", "a == != b"] {
            assert!(expr(VARS, &[], text).compile().is_err(), "{text}");
        }
    }

    const VALUES: [f64; 6] = [-1.0, -0.0, 0.0, 0.5, 1.0, f64::NAN];

    /// The bits of a result: 1, +0 or NaN.
    fn assert_truth(text: &str, a: f64, b: f64, got: f64, expected: Option<bool>) {
        match expected {
            None => assert!(got.is_nan(), "{text} with {a}, {b}: {got}"),
            Some(t) => assert_eq!(
                got.to_bits(),
                if t { 1.0_f64 } else { 0.0_f64 }.to_bits(),
                "{text} with {a}, {b}: {got}"
            ),
        }
    }

    #[test]
    fn comparisons_and_logical_operators_give_1_and_plus_0_and_pass_nan_on() {
        type Rule = fn(f64, f64) -> bool;
        let comparisons: [(&str, Rule); 6] = [
            ("a < b", |a, b| a < b),
            ("a <= b", |a, b| a <= b),
            ("a > b", |a, b| a > b),
            ("a >= b", |a, b| a >= b),
            ("a == b", |a, b| a == b),
            ("a != b", |a, b| a != b),
        ];
        let logical: [(&str, Rule); 2] = [
            ("a && b", |a, b| a != 0.0 && b != 0.0),
            ("a || b", |a, b| a != 0.0 || b != 0.0),
        ];
        for (text, rule) in comparisons.iter().chain(&logical) {
            let c = expr(&["a", "b"], &[], text).compile().unwrap();
            for a in VALUES {
                for b in VALUES {
                    let expected = (!a.is_nan() && !b.is_nan()).then(|| rule(a, b));
                    assert_truth(text, a, b, c.eval(&[a, b]), expected);
                }
            }
        }
        let not = expr(&["a"], &[], "!a").compile().unwrap();
        for a in VALUES {
            let expected = (!a.is_nan()).then_some(a == 0.0);
            assert_truth("!a", a, a, not.eval(&[a]), expected);
        }
        // -0 is false, and equal to 0; the infinities are true.
        let c = expr(&["a"], &[], "(a == 0) + !a + (a && 1)")
            .compile()
            .unwrap();
        assert_eq!(c.eval(&[-0.0]), 2.0);
        assert_eq!(c.eval(&[f64::INFINITY]), 1.0);
        assert_eq!(c.eval(&[f64::NEG_INFINITY]), 1.0);
        // == compares exactly.
        let c = expr(&[], &[], "0.1 + 0.2 == 0.3").compile().unwrap();
        assert_eq!(c.eval(&[]), 0.0);
    }

    #[test]
    fn both_sides_of_and_and_or_are_evaluated() {
        // No short circuit: a NaN on the right gives NaN even where the left
        // side alone decides the result in C.
        let c = expr(&["x"], &[], "(x >= 0) && (sqrt(x) > 1)")
            .compile()
            .unwrap();
        assert!(c.eval(&[-4.0]).is_nan());
        assert_eq!(c.eval(&[4.0]), 1.0);
        assert_eq!(c.eval(&[0.25]), 0.0);
        let c = expr(&["x"], &[], "(x < 0) || (ln(x) > 0)")
            .compile()
            .unwrap();
        assert!(c.eval(&[-1.0]).is_nan());
        // The guard on the value itself.
        let c = expr(&["x"], &[], "(x >= 0) && (sqrt(max(x, 0)) > 1)")
            .compile()
            .unwrap();
        assert_eq!(c.eval(&[-4.0]).to_bits(), 0.0_f64.to_bits());
    }

    fn eval1(text: &str, x: f64) -> f64 {
        expr(&["x"], &[], text).compile().unwrap().eval(&[x])
    }

    #[track_caller]
    fn assert_bits(text: &str, x: f64, expected: f64) {
        let got = eval1(text, x);
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "{text} at {x:e}: {got:e}, expected {expected:e}"
        );
    }

    #[test]
    fn acos_and_asin_have_the_closed_form_values_and_nan_beyond_one() {
        use std::f64::consts::{FRAC_PI_2, PI};
        assert_bits("acos(x)", 1.0, 0.0);
        assert_bits("acos(x)", -1.0, PI);
        assert_bits("acos(x)", 0.0, FRAC_PI_2);
        assert_bits("asin(x)", 1.0, FRAC_PI_2);
        assert_bits("asin(x)", -1.0, -FRAC_PI_2);
        assert_bits("asin(x)", 0.0, 0.0);
        assert_bits("asin(x)", -0.0, -0.0);
        let beyond = 1.0 + f64::EPSILON;
        for text in ["acos(x)", "asin(x)"] {
            for x in [beyond, -beyond, 2.0, f64::INFINITY, f64::NAN] {
                assert!(eval1(text, x).is_nan(), "{text} at {x}");
            }
            // The guard of the hint of E251 brings it back to the edge.
            let guarded = text.replace("(x)", "(min(max(x, -1), 1))");
            assert!(eval1(&guarded, beyond).is_finite(), "{guarded}");
        }
    }

    #[test]
    fn floor_ceil_trunc_and_round_follow_c() {
        let below_half = 0.499_999_999_999_999_94_f64; // the largest double below 0.5
        assert!(below_half < 0.5 && below_half + f64::EPSILON / 4.0 >= 0.5);
        let big = 4_503_599_627_370_495.5_f64; // 2^52 - 0.5
                                               // x, floor, ceil, trunc, round (halves away from 0)
        let table = [
            (0.0, 0.0, 0.0, 0.0, 0.0),
            (-0.0, -0.0, -0.0, -0.0, -0.0),
            (0.5, 0.0, 1.0, 0.0, 1.0),
            (-0.5, -1.0, -0.0, -0.0, -1.0),
            (1.5, 1.0, 2.0, 1.0, 2.0),
            (-1.5, -2.0, -1.0, -1.0, -2.0),
            (2.5, 2.0, 3.0, 2.0, 3.0),
            (-2.5, -3.0, -2.0, -2.0, -3.0),
            (-1.7, -2.0, -1.0, -1.0, -2.0),
            (below_half, 0.0, 1.0, 0.0, 0.0),
            (-below_half, -1.0, -0.0, -0.0, -0.0),
            (big, big - 0.5, big + 0.5, big - 0.5, big + 0.5),
            (-big, -big - 0.5, -big + 0.5, -big + 0.5, -big - 0.5),
            (
                9_007_199_254_740_992.0,
                9_007_199_254_740_992.0,
                9_007_199_254_740_992.0,
                9_007_199_254_740_992.0,
                9_007_199_254_740_992.0,
            ),
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ),
        ];
        for (x, floor, ceil, trunc, round) in table {
            assert_bits("floor(x)", x, floor);
            assert_bits("ceil(x)", x, ceil);
            assert_bits("trunc(x)", x, trunc);
            assert_bits("round(x)", x, round);
        }
        // floor(x + 0.5) is not round: the sum rounds up to 1.
        assert_bits("floor(x + 0.5)", below_half, 1.0);
        for text in ["floor(x)", "ceil(x)", "trunc(x)", "round(x)"] {
            assert!(eval1(text, f64::NAN).is_nan(), "{text}");
        }
        // The wrap of a phase into [0, 2 pi) and [-pi, pi], as in the docs.
        let wrap = expr(&["x"], &[], "x - 2*pi*floor(x/(2*pi))")
            .compile()
            .unwrap();
        assert!((wrap.eval(&[-0.5]) - (2.0 * std::f64::consts::PI - 0.5)).abs() < 1e-15);
        let wrap = expr(&["x"], &[], "x - 2*pi*round(x/(2*pi))")
            .compile()
            .unwrap();
        assert!((wrap.eval(&[7.0]) - (7.0 - 2.0 * std::f64::consts::PI)).abs() < 1e-15);
        for bad in ["floor()", "round(x, x)", "acos(x, 1)"] {
            assert!(expr(&["x"], &[], bad).compile().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_operators_and_functions_of_0_10_are_listed_as_additions() {
        let c = expr(VARS, &[], "!(a >= b) || floor(c) == d")
            .compile()
            .unwrap();
        let listed: Vec<(&str, &str, usize)> = c
            .additions()
            .iter()
            .map(|a| (a.version, a.what.as_str(), a.column))
            .collect();
        assert_eq!(
            listed,
            [
                ("0.10", "'!'", 1),
                ("0.10", "'>='", 5),
                ("0.10", "'||'", 11),
                ("0.10", "floor", 14),
                ("0.10", "'=='", 23)
            ]
        );
        assert!(expr(VARS, &[], "sin(a) - b^2")
            .compile()
            .unwrap()
            .additions()
            .is_empty());
    }

    #[test]
    fn the_new_function_names_are_reserved_only_from_0_10_on() {
        // In 0.9, round and floor are free for signals and variables.
        let old = expr(&["round"], &[("floor", "f.x")], "2*round + floor");
        let c = old.compile_for("0.9").unwrap();
        assert_eq!(c.resolved_signals(), ["round", "f.x"]);
        assert_eq!(c.eval(&[1.5, 3.0]), 6.0);
        assert!(c.additions().is_empty());
        // ... and a call of such a signal is an unknown function, as before.
        let err = expr(&["round", "x"], &[], "round(x)")
            .compile_for("0.9")
            .unwrap_err();
        assert!(
            err.message.contains("unknown function \"round\""),
            "{}",
            err.message
        );
        assert!(!err.hint.contains("floor"), "{}", err.hint);
        // In 0.10 they are built-in functions.
        let err = old.compile_for("0.10").unwrap_err();
        assert_eq!(err.field_path(), "variables.floor");
        assert!(err.message.contains("collides"), "{}", err.message);
        let err = expr(&["round"], &[], "2*round")
            .compile_for("0.10")
            .unwrap_err();
        assert!(
            err.message.contains("without parentheses"),
            "{}",
            err.message
        );
        let err = expr(&["x"], &[], "foo(x)").compile_for("0.10").unwrap_err();
        assert!(
            err.hint.contains("acos, asin, floor, ceil, round, trunc"),
            "{}",
            err.hint
        );
        // In 0.9 a call of a function of 0.10 compiles and is listed (for
        // E102), and so does an operator.
        let c = expr(&["x"], &[], "acos(x)").compile_for("0.9").unwrap();
        assert_eq!(c.additions()[0].what, "acos");
        let c = expr(&["x"], &[], "x > 0").compile_for("0.8").unwrap();
        assert_eq!(c.additions()[0].what, "'>'");
        // A version that is not supported reads like the oldest ones.
        assert!(expr(&["round"], &[], "round").compile_for("9.9").is_ok());
    }

    #[test]
    fn deep_logical_nesting_stays_within_the_limits() {
        let nots = format!("{}x", "!".repeat(MAX_PARSE_DEPTH));
        assert_eq!(
            expr(&["x"], &[], &nots).compile().unwrap().eval(&[0.0]),
            0.0
        );
        let too_many = format!("{}x", "!".repeat(MAX_PARSE_DEPTH + 1));
        assert!(expr(&["x"], &[], &too_many).compile().is_err());
        // 70 comparisons joined by &&, nested to the right: more than 64
        // stack slots.
        let mut text = String::new();
        for _ in 0..70 {
            text.push_str("x < 1 && (");
        }
        text.push_str("x < 1");
        text.push_str(&")".repeat(70));
        let c = expr(&["x"], &[], &text).compile().unwrap();
        assert_eq!(c.eval(&[0.0]), 1.0);
        assert_eq!(c.eval(&[2.0]), 0.0);
    }
}
