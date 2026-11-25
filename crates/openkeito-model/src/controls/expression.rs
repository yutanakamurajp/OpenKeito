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

const BUILTIN_FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "atan", "atan2", "abs", "sqrt", "exp", "ln", "sign", "min", "max",
    "hypot", "pow",
];

/// Scalar mathematical expression of the input signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub name: String,
    /// Input signal names. Signals that are valid identifiers `[A-Za-z_][A-Za-z0-9_]*`
    /// and not built-in function names can be referenced directly; any input can
    /// also be referenced positionally as `u0`, `u1`, ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// Mapping from expression variable names (`[A-Za-z_][A-Za-z0-9_]*`) to
    /// signal names, for signals containing `.` (such as `v.d`) or aliases.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    /// Mathematical expression string.
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
}

/// Compiled bytecode representation of an [`Expression`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledExpression {
    ops: Vec<ExprOp>,
    resolved_signals: Vec<String>,
    max_depth: usize,
}

impl CompiledExpression {
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
    /// [`CompiledExpression`].
    pub fn compile(&self) -> Result<CompiledExpression, ExprError> {
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
            if is_valid_ident(sig) && !is_builtin_fn(sig) {
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
            if is_builtin_fn(var_name) {
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
            input_idents: &input_idents,
            signal_slots: HashMap::new(),
            resolved_signals: Vec::new(),
            ops: Vec::new(),
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

    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        if let Err(err) = self.compile() {
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
                      exp, ln, sign, min, max, hypot, pow. Angles are in radians. A non-finite \
                      result stops the run (E251).",
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

fn is_builtin_fn(s: &str) -> bool {
    BUILTIN_FUNCTIONS.contains(&s)
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
                    hint: "Expressions allow numbers, identifiers, parentheses, commas, and \
                           operators +, -, *, /, ^."
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
    input_idents: &'a HashMap<&'a str, usize>,
    signal_slots: HashMap<String, usize>,
    resolved_signals: Vec<String>,
    ops: Vec<ExprOp>,
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

    fn parse_expr(&mut self, depth: usize) -> Result<(), ExprError> {
        self.parse_add_sub(depth)
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
                hint: "Expected a number, variable, function call, unary +/-, or '('."
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
            _ => {
                return Err(ExprError {
                    location: ExprErrorLocation::Expression { column: name_col },
                    message: format!("unknown function \"{name}\" at character {name_col}"),
                    hint: format!(
                        "Supported functions: {}. If \"{name}\" is a variable, put an operator \
                         like '*' before '('.",
                        BUILTIN_FUNCTIONS.join(", ")
                    ),
                });
            }
        };
        self.ops.push(op);
        Ok(())
    }

    fn resolve_ident(&mut self, name: &str, column: usize) -> Result<(), ExprError> {
        if is_builtin_fn(name) {
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
            if is_valid_ident(sig) && !is_builtin_fn(sig) {
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
            | ExprOp::Sign => {}
            ExprOp::Add
            | ExprOp::Sub
            | ExprOp::Mul
            | ExprOp::Div
            | ExprOp::Pow
            | ExprOp::Atan2
            | ExprOp::Hypot => {
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
}
