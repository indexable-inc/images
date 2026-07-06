//! The `.efx` surface language.
//!
//! A deliberately *total* frontend for [`efx_ir`]: no loops, no recursion, no
//! conditionals — a file is a finite list of `let` bindings and `effect`
//! declarations, and bindings may only mention earlier bindings, so
//! compilation terminates by construction. Anything Turing-complete can emit
//! the IR directly; this language exists for the plans that don't need it.
//!
//! ```text
//! let title = "efx demo"
//!
//! effect page "html.render" {
//!   template = "<h1>{title}</h1>"
//!   title = title
//! }
//!
//! effect site "file.write" {
//!   @rollback = "delete the file"
//!   path = "out/index.html"
//!   content = ref("page").html
//! }
//! ```
//!
//! Strings interpolate `let` bindings with `{name}` (`{{`/`}}` for literal
//! braces); `ref("effect").field` wires an upstream output into an input;
//! `@idempotent` / `@rollback` set effect metadata.

use std::collections::BTreeMap;

use efx_ir::{Effect, EffectMeta, Literal, OutputRef, Plan, Value};
use snafu::Snafu;

/// A compile failure, located in the source.
#[derive(Debug, Snafu)]
#[snafu(display("{line}:{col}: {message}"))]
pub struct CompileError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl CompileError {
    fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            col,
            message: message.into(),
        }
    }

    /// The error with its offending source line and a caret, for CLIs.
    #[must_use]
    pub fn render(&self, source: &str) -> String {
        source.lines().nth(self.line.saturating_sub(1)).map_or_else(
            || self.to_string(),
            |text| {
                let caret = " ".repeat(self.col.saturating_sub(1));
                format!("{self}\n  {text}\n  {caret}^")
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Ident(String),
    Str(String),
    Int(i64),
    Let,
    Effect,
    Ref,
    True,
    False,
    Eq,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Dot,
    At,
    Eof,
}

impl TokenKind {
    fn describe(&self) -> String {
        match self {
            Self::Ident(name) => format!("identifier `{name}`"),
            Self::Str(_) => "string literal".to_owned(),
            Self::Int(n) => format!("integer `{n}`"),
            Self::Let => "`let`".to_owned(),
            Self::Effect => "`effect`".to_owned(),
            Self::Ref => "`ref`".to_owned(),
            Self::True => "`true`".to_owned(),
            Self::False => "`false`".to_owned(),
            Self::Eq => "`=`".to_owned(),
            Self::LBrace => "`{`".to_owned(),
            Self::RBrace => "`}`".to_owned(),
            Self::LParen => "`(`".to_owned(),
            Self::RParen => "`)`".to_owned(),
            Self::Dot => "`.`".to_owned(),
            Self::At => "`@`".to_owned(),
            Self::Eof => "end of file".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    line: usize,
    col: usize,
}

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().peekable(),
            line: 1,
            col: 1,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn tokens(mut self) -> Result<Vec<Token>, CompileError> {
        let mut tokens = Vec::new();
        loop {
            while let Some(&c) = self.chars.peek() {
                if c == '#' {
                    while self.chars.peek().is_some_and(|&c| c != '\n') {
                        self.bump();
                    }
                } else if c.is_whitespace() {
                    self.bump();
                } else {
                    break;
                }
            }
            let (line, col) = (self.line, self.col);
            let Some(c) = self.bump() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    line,
                    col,
                });
                return Ok(tokens);
            };
            let kind = match c {
                '=' => TokenKind::Eq,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '.' => TokenKind::Dot,
                '@' => TokenKind::At,
                '"' => self.string(line, col)?,
                c if c.is_ascii_digit() || c == '-' => self.number(c, line, col)?,
                c if c.is_alphabetic() || c == '_' => {
                    let mut word = String::from(c);
                    while self
                        .chars
                        .peek()
                        .is_some_and(|&c| c.is_alphanumeric() || c == '_')
                    {
                        word.push(self.bump().unwrap_or_default());
                    }
                    match word.as_str() {
                        "let" => TokenKind::Let,
                        "effect" => TokenKind::Effect,
                        "ref" => TokenKind::Ref,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        _ => TokenKind::Ident(word),
                    }
                }
                other => {
                    return Err(CompileError::new(
                        line,
                        col,
                        format!("unexpected character `{other}`"),
                    ));
                }
            };
            tokens.push(Token { kind, line, col });
        }
    }

    /// Consumes a string body after the opening quote. Escapes: `\"`, `\\`,
    /// `\n`, `\t`. Interpolation braces stay raw for the compile phase.
    fn string(&mut self, line: usize, col: usize) -> Result<TokenKind, CompileError> {
        let mut text = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(CompileError::new(line, col, "unterminated string"));
            };
            match c {
                '"' => return Ok(TokenKind::Str(text)),
                '\n' => return Err(CompileError::new(line, col, "unterminated string")),
                '\\' => {
                    let (escape_line, escape_col) = (self.line, self.col);
                    match self.bump() {
                        Some('"') => text.push('"'),
                        Some('\\') => text.push('\\'),
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        other => {
                            let shown = other
                                .map_or_else(|| "end of file".to_owned(), |c| format!("`\\{c}`"));
                            return Err(CompileError::new(
                                escape_line,
                                escape_col,
                                format!("unknown escape {shown} (supported: \\\" \\\\ \\n \\t)"),
                            ));
                        }
                    }
                }
                other => text.push(other),
            }
        }
    }

    fn number(&mut self, first: char, line: usize, col: usize) -> Result<TokenKind, CompileError> {
        let mut text = String::from(first);
        while self.chars.peek().is_some_and(char::is_ascii_digit) {
            text.push(self.bump().unwrap_or_default());
        }
        text.parse::<i64>()
            .map(TokenKind::Int)
            .map_err(|_| CompileError::new(line, col, format!("`{text}` is not a valid integer")))
    }
}

#[derive(Clone, Debug)]
enum Expr {
    Str(String),
    Int(i64),
    Bool(bool),
    Var(String),
    Ref { effect: String, field: String },
}

struct Spanned<T> {
    node: T,
    line: usize,
    col: usize,
}

struct EffectDecl {
    name: Spanned<String>,
    kind: String,
    inputs: Vec<(Spanned<String>, Spanned<Expr>)>,
    props: Vec<(Spanned<String>, Spanned<Expr>)>,
}

enum Item {
    Let {
        name: Spanned<String>,
        value: Spanned<Expr>,
    },
    Effect(EffectDecl),
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.position.min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        self.position = (self.position + 1).min(self.tokens.len() - 1);
        token
    }

    fn error(token: &Token, message: impl Into<String>) -> CompileError {
        CompileError::new(token.line, token.col, message)
    }

    fn expect(&mut self, kind: &TokenKind, context: &str) -> Result<Token, CompileError> {
        let token = self.bump();
        if token.kind == *kind {
            Ok(token)
        } else {
            Err(Self::error(
                &token,
                format!(
                    "expected {} {context}, found {}",
                    kind.describe(),
                    token.kind.describe()
                ),
            ))
        }
    }

    fn ident(&mut self, context: &str) -> Result<Spanned<String>, CompileError> {
        let token = self.bump();
        let (line, col) = (token.line, token.col);
        match token.kind {
            TokenKind::Ident(name) => Ok(Spanned {
                node: name,
                line,
                col,
            }),
            other => Err(CompileError::new(
                line,
                col,
                format!("expected a name {context}, found {}", other.describe()),
            )),
        }
    }

    fn file(&mut self) -> Result<Vec<Item>, CompileError> {
        let mut items = Vec::new();
        loop {
            let token = self.bump();
            let (line, col) = (token.line, token.col);
            match token.kind {
                TokenKind::Eof => return Ok(items),
                TokenKind::Let => {
                    let name = self.ident("after `let`")?;
                    self.expect(&TokenKind::Eq, "after the binding name")?;
                    let value = self.expr()?;
                    items.push(Item::Let { name, value });
                }
                TokenKind::Effect => items.push(Item::Effect(self.effect()?)),
                other => {
                    return Err(CompileError::new(
                        line,
                        col,
                        format!(
                            "expected `let`, `effect`, or end of file, found {}",
                            other.describe()
                        ),
                    ));
                }
            }
        }
    }

    fn effect(&mut self) -> Result<EffectDecl, CompileError> {
        let name = self.ident("after `effect`")?;
        let kind_token = self.bump();
        let TokenKind::Str(kind) = kind_token.kind else {
            return Err(Self::error(
                &kind_token,
                format!(
                    "expected the effect kind as a string after `effect {}`, found {}",
                    name.node,
                    kind_token.kind.describe()
                ),
            ));
        };
        self.expect(&TokenKind::LBrace, "to open the effect body")?;
        let mut inputs = Vec::new();
        let mut props = Vec::new();
        loop {
            if self.peek().kind == TokenKind::RBrace {
                self.bump();
                return Ok(EffectDecl {
                    name,
                    kind,
                    inputs,
                    props,
                });
            }
            if self.peek().kind == TokenKind::At {
                self.bump();
                let key = self.ident("after `@`")?;
                self.expect(&TokenKind::Eq, "after the property name")?;
                props.push((key, self.expr()?));
            } else {
                let key = self.ident("as the input name (or `}` to close the effect)")?;
                self.expect(&TokenKind::Eq, "after the input name")?;
                inputs.push((key, self.expr()?));
            }
        }
    }

    fn expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let token = self.bump();
        let (line, col) = (token.line, token.col);
        let spanned = move |node| Spanned { node, line, col };
        match token.kind {
            TokenKind::Str(text) => Ok(spanned(Expr::Str(text))),
            TokenKind::Int(n) => Ok(spanned(Expr::Int(n))),
            TokenKind::True => Ok(spanned(Expr::Bool(true))),
            TokenKind::False => Ok(spanned(Expr::Bool(false))),
            TokenKind::Ident(name) => Ok(spanned(Expr::Var(name))),
            TokenKind::Ref => {
                self.expect(&TokenKind::LParen, "after `ref`")?;
                let target = self.bump();
                let TokenKind::Str(effect) = target.kind else {
                    return Err(Self::error(
                        &target,
                        format!(
                            "expected an effect name string inside `ref(...)`, found {}",
                            target.kind.describe()
                        ),
                    ));
                };
                self.expect(&TokenKind::RParen, "to close `ref(...)`")?;
                self.expect(&TokenKind::Dot, "after `ref(...)` (which output field?)")?;
                let field = self.ident("as the output field after `.`")?;
                Ok(spanned(Expr::Ref {
                    effect,
                    field: field.node,
                }))
            }
            other => Err(CompileError::new(
                line,
                col,
                format!(
                    "expected a value (string, integer, boolean, binding, or `ref(...)`), found {}",
                    other.describe()
                ),
            )),
        }
    }
}

/// Substitutes `{name}` interpolations from `bindings`; `{{` and `}}` are
/// literal braces.
fn interpolate(
    raw: &str,
    bindings: &BTreeMap<String, Literal>,
    line: usize,
    col: usize,
) -> Result<String, CompileError> {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) if c.is_alphanumeric() || c == '_' => name.push(c),
                        _ => {
                            return Err(CompileError::new(
                                line,
                                col,
                                "unclosed `{` in string; use `{{` for a literal brace",
                            ));
                        }
                    }
                }
                let value = bindings.get(&name).ok_or_else(|| {
                    CompileError::new(
                        line,
                        col,
                        format!(
                            "string interpolates `{{{name}}}`, but no `let {name}` precedes it"
                        ),
                    )
                })?;
                output.push_str(&value.display_string());
            }
            '}' => {
                return Err(CompileError::new(
                    line,
                    col,
                    "stray `}` in string; use `}}` for a literal brace",
                ));
            }
            other => output.push(other),
        }
    }
    Ok(output)
}

fn eval(expr: &Spanned<Expr>, bindings: &BTreeMap<String, Literal>) -> Result<Value, CompileError> {
    match &expr.node {
        Expr::Str(raw) => Ok(Value::Literal(Literal::Str(interpolate(
            raw, bindings, expr.line, expr.col,
        )?))),
        Expr::Int(n) => Ok(Value::Literal(Literal::Int(*n))),
        Expr::Bool(b) => Ok(Value::Literal(Literal::Bool(*b))),
        Expr::Var(name) => bindings
            .get(name)
            .map(|lit| Value::Literal(lit.clone()))
            .ok_or_else(|| {
                CompileError::new(
                    expr.line,
                    expr.col,
                    format!("`{name}` is not defined by an earlier `let`"),
                )
            }),
        Expr::Ref { effect, field } => Ok(Value::Ref(OutputRef {
            effect: effect.clone(),
            field: field.clone(),
        })),
    }
}

/// Compiles `.efx` source to an IR plan.
///
/// # Errors
///
/// Returns a located [`CompileError`] for lexical, syntactic, and semantic
/// failures (unknown bindings, duplicate names, references to undeclared
/// effects, and so on).
pub fn compile(source: &str) -> Result<Plan, CompileError> {
    let tokens = Lexer::new(source).tokens()?;
    let items = Parser {
        tokens,
        position: 0,
    }
    .file()?;

    let mut bindings: BTreeMap<String, Literal> = BTreeMap::new();
    let mut plan = Plan::new();
    let mut effect_sites: BTreeMap<String, Spanned<String>> = BTreeMap::new();
    for item in items {
        match item {
            Item::Let { name, value } => {
                if bindings.contains_key(&name.node) {
                    return Err(CompileError::new(
                        name.line,
                        name.col,
                        format!("`{}` is already bound by an earlier `let`", name.node),
                    ));
                }
                let Value::Literal(literal) = eval(&value, &bindings)? else {
                    return Err(CompileError::new(
                        value.line,
                        value.col,
                        "`let` bindings hold literals; `ref(...)` belongs in effect inputs",
                    ));
                };
                bindings.insert(name.node, literal);
            }
            Item::Effect(decl) => {
                let mut inputs = BTreeMap::new();
                for (key, value) in &decl.inputs {
                    if inputs.contains_key(&key.node) {
                        return Err(CompileError::new(
                            key.line,
                            key.col,
                            format!("duplicate input `{}`", key.node),
                        ));
                    }
                    inputs.insert(key.node.clone(), eval(value, &bindings)?);
                }
                let meta = effect_meta(&decl, &bindings)?;
                let site = Spanned {
                    node: decl.name.node.clone(),
                    line: decl.name.line,
                    col: decl.name.col,
                };
                if plan
                    .add(Effect {
                        name: decl.name.node.clone(),
                        kind: decl.kind.clone(),
                        executor: decl.kind,
                        inputs,
                        meta,
                    })
                    .is_err()
                {
                    return Err(CompileError::new(
                        decl.name.line,
                        decl.name.col,
                        format!("an effect named `{}` already exists", decl.name.node),
                    ));
                }
                effect_sites.insert(site.node.clone(), site);
            }
        }
    }

    // Refs are checked here rather than left to the engine so the error
    // carries a source location.
    for effect in plan.effects() {
        for value in effect.inputs.values() {
            if let Value::Ref(r) = value
                && !effect_sites.contains_key(&r.effect)
            {
                let site = &effect_sites[&effect.name];
                return Err(CompileError::new(
                    site.line,
                    site.col,
                    format!(
                        "effect `{}` references `ref(\"{}\")`, which is not declared",
                        effect.name, r.effect
                    ),
                ));
            }
        }
    }
    Ok(plan)
}

fn effect_meta(
    decl: &EffectDecl,
    bindings: &BTreeMap<String, Literal>,
) -> Result<EffectMeta, CompileError> {
    let mut meta = EffectMeta::default();
    for (key, value) in &decl.props {
        let evaluated = eval(value, bindings)?;
        match (key.node.as_str(), evaluated) {
            ("idempotent", Value::Literal(Literal::Bool(b))) => meta.idempotent = b,
            ("idempotent", _) => {
                return Err(CompileError::new(
                    value.line,
                    value.col,
                    "`@idempotent` takes `true` or `false`",
                ));
            }
            ("rollback", Value::Literal(Literal::Str(hint))) => meta.rollback_hint = Some(hint),
            ("rollback", _) => {
                return Err(CompileError::new(
                    value.line,
                    value.col,
                    "`@rollback` takes a string hint",
                ));
            }
            (other, _) => {
                return Err(CompileError::new(
                    key.line,
                    key.col,
                    format!("unknown property `@{other}` (supported: @idempotent, @rollback)"),
                ));
            }
        }
    }
    Ok(meta)
}
