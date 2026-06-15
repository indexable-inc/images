//! Recursive-descent parser for the query DSL, mirroring the structure of
//! flecs's `addons/query_dsl/parser.c` so behavior can be audited
//! function-by-function against upstream.

use crate::ast::{
    Access, EqOp, EqOperand, EqTerm, ExtraOper, IdFlag, IdTerm, Oper, Query, Ref, RefExpr, Src,
    Term, TermBody, Traversal,
};
use crate::error::{ParseError, Span};
use crate::token::{Token, TokenKind, lex};

/// Upstream `FLECS_TERM_ARG_COUNT_MAX`: the most pair targets one term can
/// carry beyond `(src, second)`.
const TERM_ARG_COUNT_MAX: usize = 16;

/// How deep `{ ... }` scopes may nest. The parser recurses once per scope,
/// so without a bound a long `{{{{...` run from untrusted input overflows
/// the stack, which aborts the process rather than panicking.
const SCOPE_DEPTH_MAX: usize = 64;

/// Parse a query expression into its AST.
///
/// The expression `0` parses to the empty query, exactly as in flecs.
///
/// # Errors
/// Returns a [`ParseError`] with a byte span when the expression is not
/// well-formed. Identifier *resolution* (does `Position` exist?) is a
/// separate, world-dependent concern this parser deliberately stays out of.
pub fn parse(src: &str) -> Result<Query, ParseError> {
    if src == "0" {
        return Ok(Query { terms: Vec::new() });
    }
    let tokens = lex(src)?;
    let mut parser = Parser {
        tokens,
        index: 0,
        scope_depth: 0,
    };
    let terms = parser.terms(ScopeContext::TopLevel)?;
    Ok(Query { terms })
}

/// Whether a term list ends at end-of-input or at a closing `}`.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ScopeContext {
    TopLevel,
    Scope,
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    scope_depth: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.index]
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.index].clone();
        if self.index + 1 < self.tokens.len() {
            self.index += 1;
        }
        token
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> Result<Token, ParseError> {
        let token = self.bump();
        if token.kind == kind {
            Ok(token)
        } else {
            unexpected(&token, expected)
        }
    }

    /// Parse a term list until end-of-input (or `}` inside a scope),
    /// handling the `,` and `||` separators between terms.
    fn terms(&mut self, context: ScopeContext) -> Result<Vec<Term>, ParseError> {
        let mut terms = Vec::new();

        loop {
            match self.peek().kind {
                TokenKind::Eof => {
                    if context == ScopeContext::Scope {
                        let token = self.peek().clone();
                        return unexpected(&token, "'}'");
                    }
                    return Ok(terms);
                }
                TokenKind::RBrace if context == ScopeContext::Scope => {
                    self.bump();
                    return Ok(terms);
                }
                _ => {}
            }

            terms.push(self.term()?);

            match self.peek().kind {
                TokenKind::Comma => {
                    self.bump();
                }
                TokenKind::OrOr => {
                    let or = self.bump();
                    let previous = terms.last_mut().expect("a term was just pushed");
                    if previous.oper != Oper::And {
                        return Err(ParseError::new(
                            "cannot mix operators in || expression",
                            or.span,
                        ));
                    }
                    previous.oper = Oper::Or;
                    // Upstream leaves a trailing `||` to the validator;
                    // reject it here since the chain is plainly incomplete.
                    if matches!(self.peek().kind, TokenKind::Eof | TokenKind::RBrace) {
                        return Err(ParseError::new("expected term after '||'", or.span));
                    }
                }
                TokenKind::Eof | TokenKind::RBrace => {}
                _ => {
                    let token = self.peek().clone();
                    return unexpected(&token, "',' or end of expression");
                }
            }
        }
    }

    /// Parse one term: optional `[access]`, optional `!`/`?`, then the body.
    fn term(&mut self) -> Result<Term, ParseError> {
        let access = self.access()?;

        let mut oper = match self.peek().kind {
            TokenKind::Bang => {
                self.bump();
                Oper::Not
            }
            TokenKind::Question => {
                self.bump();
                Oper::Optional
            }
            _ => Oper::And,
        };
        let had_prefix = oper != Oper::And;

        let body = match self.peek().kind {
            // A scope is reachable bare or after `!`/`?`, but not after an
            // access modifier, exactly as upstream.
            TokenKind::LBrace if access.is_none() => {
                let brace = self.bump();
                self.scope_depth += 1;
                if self.scope_depth > SCOPE_DEPTH_MAX {
                    return Err(ParseError::new(
                        format!("scopes nested deeper than {SCOPE_DEPTH_MAX}"),
                        brace.span,
                    ));
                }
                let inner = self.terms(ScopeContext::Scope)?;
                self.scope_depth -= 1;
                TermBody::Scope(inner)
            }
            TokenKind::LParen => {
                self.bump();
                self.pair_body()?
            }
            TokenKind::Ident | TokenKind::Number | TokenKind::Star => {
                // Keyword forms (`and|`, `toggle|`, ...) only apply where
                // upstream consults them: terms without a `!`/`?` prefix.
                let keyword = if had_prefix {
                    None
                } else {
                    self.keyword()?
                };
                match keyword {
                    Some(KeywordPrefix::Oper(keyword_oper)) => {
                        oper = keyword_oper;
                        self.keyword_operand(None, &mut oper)?
                    }
                    Some(KeywordPrefix::Flag(flag)) => self.keyword_operand(Some(flag), &mut oper)?,
                    None => {
                        let first = self.ref_expr()?;
                        self.id_body(first, &mut oper)?
                    }
                }
            }
            _ => {
                let token = self.peek().clone();
                return unexpected(&token, "term");
            }
        };

        Ok(Term { access, oper, body })
    }

    /// Parse the `[access]` prefix, when present.
    fn access(&mut self) -> Result<Option<Access>, ParseError> {
        if self.peek().kind != TokenKind::LBracket {
            return Ok(None);
        }
        self.bump();
        let word = self.expect(TokenKind::Ident, "access modifier")?;
        let access = match word.text.as_str() {
            "default" => Access::Default,
            "in" => Access::In,
            "out" => Access::Out,
            "inout" => Access::InOut,
            "none" => Access::None,
            "filter" => Access::Filter,
            // Upstream silently ignores unknown modifiers; rejecting them is
            // a deliberate strictness improvement.
            other => {
                return Err(ParseError::new(
                    format!("invalid access modifier '{other}'"),
                    word.span,
                ));
            }
        };
        self.expect(TokenKind::RBracket, "']'")?;
        Ok(Some(access))
    }

    /// Recognize a reserved keyword (`and`, `or`, `not`, `auto_override`,
    /// `toggle`) at the start of a term body. The `|` after it is mandatory
    /// upstream, which is what makes these words reserved in this position.
    fn keyword(&mut self) -> Result<Option<KeywordPrefix>, ParseError> {
        let word = self.peek();
        if word.kind != TokenKind::Ident {
            return Ok(None);
        }
        let keyword = match word.text.as_str() {
            "and" => KeywordPrefix::Oper(Oper::AndFrom),
            "or" => KeywordPrefix::Oper(Oper::OrFrom),
            "not" => KeywordPrefix::Oper(Oper::NotFrom),
            "auto_override" => KeywordPrefix::Flag(IdFlag::AutoOverride),
            "toggle" => KeywordPrefix::Flag(IdFlag::Toggle),
            _ => return Ok(None),
        };
        self.bump();
        self.expect(TokenKind::Pipe, "'|'")?;
        Ok(Some(keyword))
    }

    /// Parse the term body following a keyword prefix: an identifier or a
    /// pair form, as upstream allows both (`and|Type`, `and|(Rel, Tgt)`).
    fn keyword_operand(
        &mut self,
        flag: Option<IdFlag>,
        oper: &mut Oper,
    ) -> Result<TermBody, ParseError> {
        let mut body = if self.peek().kind == TokenKind::LParen {
            self.bump();
            self.pair_body()?
        } else {
            let first = self.ref_expr()?;
            self.id_body(first, oper)?
        };
        if let TermBody::Id(id) = &mut body {
            id.flag = flag;
        }
        Ok(body)
    }

    /// Read an identifier-like token as a reference expression.
    fn ref_expr(&mut self) -> Result<RefExpr, ParseError> {
        let token = self.bump();
        classify(&token)
    }

    /// Continue a term whose first reference was just read: equality
    /// predicates, `|` traversal flags, and `(...)` argument lists.
    fn id_body(&mut self, first: RefExpr, oper: &mut Oper) -> Result<TermBody, ParseError> {
        match self.peek().kind {
            TokenKind::EqEq => {
                self.bump();
                self.eq_body(first, EqOp::Eq, false, oper)
            }
            TokenKind::Neq => {
                self.bump();
                self.eq_body(first, EqOp::Eq, true, oper)
            }
            TokenKind::Match => {
                self.bump();
                self.eq_body(first, EqOp::Match, false, oper)
            }
            TokenKind::Pipe => {
                self.bump();
                let traversal = self.traversal()?;
                let first = Ref {
                    expr: first,
                    traversal: Some(traversal),
                };
                if self.peek().kind == TokenKind::LParen {
                    self.bump();
                    self.args(first)
                } else {
                    Ok(TermBody::Id(IdTerm::component(first)))
                }
            }
            TokenKind::LParen => {
                self.bump();
                self.args(Ref::plain(first))
            }
            _ => Ok(TermBody::Id(IdTerm::component(Ref::plain(first)))),
        }
    }

    /// Parse the right-hand side of `==` / `!=` / `~=`. Negation moves onto
    /// the term operator (`!=` is `Not` + `Eq`, `~= "!x"` is `Not` + `Match`),
    /// which is also why these predicates reject an existing `!`/`?` prefix.
    fn eq_body(
        &mut self,
        left: RefExpr,
        op: EqOp,
        negate: bool,
        oper: &mut Oper,
    ) -> Result<TermBody, ParseError> {
        if *oper != Oper::And {
            return Err(ParseError::new(
                "cannot mix operator with equality expression",
                self.peek().span,
            ));
        }

        let token = self.bump();
        let (right, negate) = match token.kind {
            TokenKind::Ident | TokenKind::Number | TokenKind::Star => {
                (EqOperand::Ref(classify(&token)?), negate)
            }
            // The `!` negation lives inside the string because there is only
            // one `~=` operator; upstream strips it the same way.
            TokenKind::Str if op == EqOp::Match => match token.text.strip_prefix('!') {
                Some(stripped) => (EqOperand::Name(stripped.to_owned()), true),
                None => (EqOperand::Name(token.text), negate),
            },
            TokenKind::Str => (EqOperand::Name(token.text), negate),
            _ => return unexpected(&token, "identifier or string"),
        };

        if negate {
            *oper = Oper::Not;
        }
        Ok(TermBody::Eq(EqTerm { left, op, right }))
    }

    /// Parse a `(First, second)` pair-form term (implicit `$this` source).
    fn pair_body(&mut self) -> Result<TermBody, ParseError> {
        let first_token = self.bump();
        let first = match first_token.kind {
            TokenKind::Ident | TokenKind::Number | TokenKind::Star => classify(&first_token)?,
            _ => return unexpected(&first_token, "identifier"),
        };
        let traversal = if self.peek().kind == TokenKind::Pipe {
            self.bump();
            Some(self.traversal()?)
        } else {
            None
        };
        self.expect(TokenKind::Comma, "','")?;

        let mut id = IdTerm::component(Ref {
            expr: first,
            traversal,
        });
        self.arg_list(&mut id, 1)?;
        Ok(TermBody::Id(id))
    }

    /// Parse an explicit argument list after `First(`. The first argument is
    /// the source; later arguments are pair targets.
    fn args(&mut self, first: Ref) -> Result<TermBody, ParseError> {
        let mut id = IdTerm::component(first);

        if self.peek().kind == TokenKind::RParen {
            self.bump();
            id.src = Src::Empty;
            return Ok(TermBody::Id(id));
        }

        self.arg_list(&mut id, 0)?;
        Ok(TermBody::Id(id))
    }

    /// Parse `arg (, arg)* )` starting at the given argument index
    /// (0 = source, 1 = second, 2+ = extra targets), enforcing the upstream
    /// rule that extra targets may not mix `,` and `||` separators.
    fn arg_list(&mut self, id: &mut IdTerm, first_index: usize) -> Result<(), ParseError> {
        let mut index = first_index;
        loop {
            if index > TERM_ARG_COUNT_MAX {
                return Err(ParseError::new(
                    "too many arguments in term",
                    self.peek().span,
                ));
            }
            let reference = self.arg_ref()?;
            match index {
                0 => id.src = Src::Explicit(reference),
                1 => id.second = Some(reference),
                _ => id.extra.push(reference),
            }

            let separator = self.bump();
            let oper = match separator.kind {
                TokenKind::RParen => return Ok(()),
                TokenKind::Comma => ExtraOper::And,
                TokenKind::OrOr => ExtraOper::Or,
                _ => return unexpected(&separator, "',' or ')'"),
            };
            if index > 1 && id.extra_oper != oper {
                return Err(ParseError::new(
                    "cannot mix operators in extra term arguments",
                    separator.span,
                ));
            }
            id.extra_oper = oper;
            index += 1;
        }
    }

    /// Parse one argument: traversal-flags-only, a `@` value operand, or an
    /// entity-like operand with optional `|` traversal flags.
    fn arg_ref(&mut self) -> Result<Ref, ParseError> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Ident if is_traversal_flag(&token.text) => {
                let traversal = self.traversal()?;
                Ok(Ref {
                    expr: RefExpr::Implied,
                    traversal: Some(traversal),
                })
            }
            TokenKind::At => {
                self.bump();
                let inner = self.bump();
                let expr = match inner.kind {
                    TokenKind::Ident | TokenKind::Number | TokenKind::Star => classify(&inner)?,
                    _ => return unexpected(&inner, "identifier"),
                };
                Ok(Ref::plain(RefExpr::Value(Box::new(expr))))
            }
            TokenKind::Ident | TokenKind::Number | TokenKind::Star => {
                self.bump();
                let expr = classify(&token)?;
                let traversal = if self.peek().kind == TokenKind::Pipe {
                    self.bump();
                    Some(self.traversal()?)
                } else {
                    None
                };
                Ok(Ref { expr, traversal })
            }
            _ => unexpected(&token, "identifier"),
        }
    }

    /// Parse `self|up|cascade|desc` flags (the next token must be a flag)
    /// followed by an optional traversal relationship name.
    fn traversal(&mut self) -> Result<Traversal, ParseError> {
        let mut traversal = Traversal::default();
        loop {
            let word = self.expect(TokenKind::Ident, "traversal flag")?;
            match word.text.as_str() {
                "self" => traversal.self_ = true,
                "up" => traversal.up = true,
                "cascade" => traversal.cascade = true,
                "desc" => traversal.desc = true,
                other => {
                    return Err(ParseError::new(
                        format!("expected traversal flag, found '{other}'"),
                        word.span,
                    ));
                }
            }
            match self.peek().kind {
                TokenKind::Pipe => {
                    self.bump();
                }
                TokenKind::Ident if !is_traversal_flag(&self.peek().text) => {
                    let rel = self.bump();
                    traversal.rel = Some(rel.text);
                    return Ok(traversal);
                }
                _ => return Ok(traversal),
            }
        }
    }
}

/// A reserved word at the start of a term body.
enum KeywordPrefix {
    Oper(Oper),
    Flag(IdFlag),
}

impl IdTerm {
    /// A plain component term: `first` with an implicit source and no pair.
    const fn component(first: Ref) -> Self {
        Self {
            flag: None,
            first,
            src: Src::Implicit,
            second: None,
            extra: Vec::new(),
            extra_oper: ExtraOper::And,
        }
    }
}

const fn is_traversal_flag(word: &str) -> bool {
    matches!(word.as_bytes(), b"self" | b"up" | b"cascade" | b"desc")
}

/// Build the standard "expected X, found Y" error for a token.
fn unexpected<T>(token: &Token, expected: &str) -> Result<T, ParseError> {
    let found = match token.kind {
        TokenKind::Ident | TokenKind::Number => {
            format!("{} '{}'", token.kind.describe(), token.text)
        }
        _ => token.kind.describe().to_owned(),
    };
    Err(ParseError::new(
        format!("expected {expected}, found {found}"),
        token.span,
    ))
}

/// Classify an identifier-like token into a [`RefExpr`], mirroring how the
/// flecs validator interprets reference names (`$this`, `$var`, `#123`, `_`,
/// `*`) — done eagerly here so the AST is typed instead of stringly.
fn classify(token: &Token) -> Result<RefExpr, ParseError> {
    match token.kind {
        TokenKind::Star => Ok(RefExpr::Wildcard),
        TokenKind::Number => entity_id(&token.text, token.span),
        TokenKind::Ident => match (token.text.as_str(), token.text.as_bytes().first()) {
            ("$this", _) => Ok(RefExpr::This),
            ("_", _) => Ok(RefExpr::Any),
            (text, Some(b'$')) => Ok(RefExpr::Var(text[1..].to_owned())),
            (text, Some(b'#')) => entity_id(&text[1..], token.span),
            _ => Ok(RefExpr::Name(token.text.clone())),
        },
        _ => Err(ParseError::new("expected identifier", token.span)),
    }
}

fn entity_id(digits: &str, span: Span) -> Result<RefExpr, ParseError> {
    digits
        .parse::<u64>()
        .map(RefExpr::Entity)
        .map_err(|_| ParseError::new(format!("invalid entity id '{digits}'"), span))
}
