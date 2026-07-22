//! The single mapping pass from the oxc `JavaScript` AST onto [`crate::nix`].
//!
//! Every `JavaScript` form either has exactly one Nix spelling or is a
//! positioned [`Error`]; there are no fallbacks.

use oxc_ast::ast;
use oxc_span::{GetSpan as _, Span};

use crate::error::Error;
use crate::nix::{
    Attr, BinaryOp, Binding, Expr, LetBinding, Module, Param, PatternField, StrPart, UnaryOp,
    is_bare_ident,
};

/// Maps a parsed ES module onto a Nix [`Module`].
///
/// # Errors
///
/// Returns a positioned [`Error`] for any `JavaScript` form without a 1:1 Nix
/// equivalent.
pub fn module(program: &ast::Program<'_>, source: &str) -> Result<Module, Error> {
    let mut mapper = Mapper {
        source,
        uses_import: false,
    };

    let mut bindings = Vec::new();
    let mut default = None;
    for statement in &program.body {
        match statement {
            ast::Statement::VariableDeclaration(declaration) => {
                mapper.const_bindings(declaration, &mut bindings)?;
            }
            ast::Statement::ExportDefaultDeclaration(export) => {
                let Some(expression) = export.declaration.as_expression() else {
                    let message = match &export.declaration {
                        ast::ExportDefaultDeclarationKind::FunctionDeclaration(_) => {
                            "`function` has no Nix equivalent; use an arrow function"
                        }
                        _ => {
                            "`export default` must be an expression; \
                             declarations have no Nix equivalent"
                        }
                    };
                    return Err(mapper.err(export.span, message));
                };
                if default.is_some() {
                    return Err(mapper.err(export.span, "duplicate `export default`"));
                }
                default = Some(mapper.expr(expression)?);
            }
            other => {
                return Err(mapper.err(
                    other.span(),
                    "a module maps to `let ... in <export default>`: only `const` \
                     declarations and one `export default` are allowed at top level",
                ));
            }
        }
    }

    let Some(body) = default else {
        return Err(mapper.err(
            Span::new(program.span.end, program.span.end),
            "module must `export default` an expression (its Nix value)",
        ));
    };
    Ok(Module {
        wrapped: mapper.uses_import,
        body: make_let(bindings, body),
    })
}

fn make_let(bindings: Vec<LetBinding>, body: Expr) -> Expr {
    if bindings.is_empty() {
        body
    } else {
        Expr::Let {
            bindings,
            body: Box::new(body),
        }
    }
}

struct Mapper<'s> {
    source: &'s str,
    /// Set when any `import()` is mapped; decides the module wrapper.
    uses_import: bool,
}

impl Mapper<'_> {
    fn err(&self, span: Span, message: impl Into<String>) -> Error {
        Error::at_offset32(span.start, self.source, message)
    }

    /// Maps one `JavaScript` expression to its Nix spelling.
    fn expr(&mut self, expression: &ast::Expression<'_>) -> Result<Expr, Error> {
        match expression {
            ast::Expression::BooleanLiteral(lit) => {
                Ok(Expr::Ident(if lit.value { "true" } else { "false" }.into()))
            }
            ast::Expression::NullLiteral(_) => Ok(Expr::Ident("null".into())),
            ast::Expression::NumericLiteral(lit) => self.number(lit),
            ast::Expression::StringLiteral(lit) => {
                Ok(Expr::Str(vec![StrPart::Lit(lit.value.to_string())]))
            }
            ast::Expression::TemplateLiteral(template) => self.template(template),
            ast::Expression::Identifier(ident) => self.ident(ident),
            ast::Expression::ArrayExpression(array) => self.array(array),
            ast::Expression::ObjectExpression(object) => self.object(object),
            ast::Expression::ArrowFunctionExpression(arrow) => self.arrow(arrow),
            ast::Expression::CallExpression(call) => self.call(call),
            ast::Expression::ImportExpression(import) => self.import(import),
            ast::Expression::StaticMemberExpression(member) => self.static_member(member),
            ast::Expression::ComputedMemberExpression(member) => self.computed_member(member),
            ast::Expression::ConditionalExpression(cond) => Ok(Expr::If {
                cond: Box::new(self.expr(&cond.test)?),
                then: Box::new(self.expr(&cond.consequent)?),
                otherwise: Box::new(self.expr(&cond.alternate)?),
            }),
            ast::Expression::LogicalExpression(logical) => self.logical(logical),
            ast::Expression::BinaryExpression(binary) => self.binary(binary),
            ast::Expression::UnaryExpression(unary) => self.unary(unary),
            ast::Expression::ParenthesizedExpression(paren) => self.expr(&paren.expression),
            ast::Expression::ChainExpression(chain) => Err(self.err(
                chain.span,
                "optional chaining maps only as `expr?.path ?? default` \
                 (Nix `expr.path or default`)",
            )),
            ast::Expression::FunctionExpression(function) => Err(self.err(
                function.span,
                "`function` has no Nix equivalent; use an arrow function",
            )),
            ast::Expression::AssignmentExpression(assign) => Err(self.err(
                assign.span,
                "assignment has no Nix equivalent; Nix values are immutable",
            )),
            other => Err(self.err(
                other.span(),
                "this JavaScript form has no Nix equivalent",
            )),
        }
    }

    fn ident(&self, ident: &ast::IdentifierReference<'_>) -> Result<Expr, Error> {
        let name = ident.name.as_str();
        if name == "undefined" {
            return Err(self.err(ident.span, "`undefined` has no Nix equivalent; use `null`"));
        }
        self.checked_name(ident.span, name)?;
        Ok(Expr::Ident(name.into()))
    }

    /// Rejects names that cannot be spelled as bare Nix identifiers
    /// (keywords like `then`, or `JavaScript` names containing `$`).
    fn checked_name(&self, span: Span, name: &str) -> Result<String, Error> {
        if is_bare_ident(name) {
            Ok(name.into())
        } else {
            Err(self.err(
                span,
                format!("`{name}` is not a valid Nix identifier"),
            ))
        }
    }

    fn number(&self, lit: &ast::NumericLiteral<'_>) -> Result<Expr, Error> {
        let Some(raw) = lit.raw.as_ref() else {
            return Err(self.err(lit.span, "numeric literal without source text"));
        };
        // JavaScript numeric separators are pure notation; drop them.
        let digits: String = raw.chars().filter(|c| *c != '_').collect();

        let radix = match digits.get(..2) {
            Some("0x" | "0X") => Some(16),
            Some("0b" | "0B") => Some(2),
            Some("0o" | "0O") => Some(8),
            _ => None,
        };
        let parsed = if let Some(radix) = radix {
            i64::from_str_radix(&digits[2..], radix).ok()
        } else if digits.contains(['.', 'e', 'E']) {
            return Ok(Expr::Float(lit.value));
        } else {
            digits.parse().ok()
        };
        parsed.map(Expr::Int).ok_or_else(|| {
            self.err(lit.span, "integer literal does not fit in a 64-bit Nix integer")
        })
    }

    fn template(&mut self, template: &ast::TemplateLiteral<'_>) -> Result<Expr, Error> {
        let mut parts = Vec::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            let Some(cooked) = &quasi.value.cooked else {
                return Err(self.err(quasi.span, "template literal has an invalid escape"));
            };
            if !cooked.is_empty() {
                parts.push(StrPart::Lit(cooked.to_string()));
            }
            if let Some(expression) = template.expressions.get(index) {
                parts.push(StrPart::Interp(self.expr(expression)?));
            }
        }
        Ok(Expr::Str(parts))
    }

    /// `[a, b]` maps to a list; spread segments left-fold with `++`.
    fn array(&mut self, array: &ast::ArrayExpression<'_>) -> Result<Expr, Error> {
        let mut operands = Vec::new();
        let mut items = Vec::new();
        for element in &array.elements {
            match element {
                ast::ArrayExpressionElement::Elision(elision) => {
                    return Err(self.err(elision.span, "array holes have no Nix equivalent"));
                }
                ast::ArrayExpressionElement::SpreadElement(spread) => {
                    if !items.is_empty() {
                        operands.push(Expr::List(std::mem::take(&mut items)));
                    }
                    operands.push(self.expr(&spread.argument)?);
                }
                other => {
                    let Some(expression) = other.as_expression() else {
                        return Err(
                            self.err(other.span(), "this array element has no Nix equivalent")
                        );
                    };
                    items.push(self.expr(expression)?);
                }
            }
        }
        if !items.is_empty() || operands.is_empty() {
            operands.push(Expr::List(items));
        }
        Ok(fold_binary(BinaryOp::Concat, operands))
    }

    /// `{ ... }` maps to an attrset; spread segments left-fold with `//`.
    fn object(&mut self, object: &ast::ObjectExpression<'_>) -> Result<Expr, Error> {
        let mut operands = Vec::new();
        let mut bindings: Vec<Binding> = Vec::new();
        for property in &object.properties {
            match property {
                ast::ObjectPropertyKind::SpreadProperty(spread) => {
                    if !bindings.is_empty() {
                        operands.push(Expr::AttrSet(std::mem::take(&mut bindings)));
                    }
                    operands.push(self.expr(&spread.argument)?);
                }
                ast::ObjectPropertyKind::ObjectProperty(property) => {
                    let binding = self.property(property)?;
                    if let Attr::Name(name) = &binding.key
                        && bindings.iter().any(|prior| {
                            matches!(&prior.key, Attr::Name(existing) if existing == name)
                        })
                    {
                        return Err(self.err(
                            property.span,
                            format!("duplicate key `{name}`: Nix attrsets reject duplicates"),
                        ));
                    }
                    bindings.push(binding);
                }
            }
        }
        if !bindings.is_empty() || operands.is_empty() {
            operands.push(Expr::AttrSet(bindings));
        }
        Ok(fold_binary(BinaryOp::Update, operands))
    }

    fn property(&mut self, property: &ast::ObjectProperty<'_>) -> Result<Binding, Error> {
        if property.kind != ast::PropertyKind::Init {
            return Err(self.err(
                property.span,
                "getters and setters have no Nix equivalent",
            ));
        }
        if property.method {
            return Err(self.err(
                property.span,
                "methods have no Nix equivalent; use an arrow-function value",
            ));
        }
        let key = if property.computed {
            let Some(expression) = property.key.as_expression() else {
                return Err(self.err(property.key.span(), "unsupported computed key"));
            };
            Attr::Dynamic(self.expr(expression)?)
        } else {
            match &property.key {
                ast::PropertyKey::StaticIdentifier(ident) => Attr::Name(ident.name.to_string()),
                ast::PropertyKey::StringLiteral(lit) => Attr::Name(lit.value.to_string()),
                other => {
                    return Err(self.err(
                        other.span(),
                        "object keys map to Nix attr names: use an identifier, \
                         a string, or a computed `[expr]` key",
                    ));
                }
            }
        };
        Ok(Binding {
            key,
            value: self.expr(&property.value)?,
        })
    }

    fn arrow(&mut self, arrow: &ast::ArrowFunctionExpression<'_>) -> Result<Expr, Error> {
        if arrow.r#async {
            return Err(self.err(arrow.span, "`async` has no Nix equivalent"));
        }
        if let Some(rest) = &arrow.params.rest {
            return Err(self.err(
                rest.span,
                "variadic rest parameters have no Nix equivalent",
            ));
        }
        if arrow.params.items.is_empty() {
            return Err(self.err(
                arrow.span,
                "Nix functions take exactly one argument; \
                 zero-parameter arrows have no Nix equivalent",
            ));
        }

        let mut body = self.arrow_body(arrow)?;
        // `(a, b) => e` curries to `a: b: e`, matching curried call mapping.
        for item in arrow.params.items.iter().rev() {
            if let Some(initializer) = &item.initializer {
                return Err(self.err(
                    initializer.span(),
                    "defaults are only expressible inside a destructured object \
                     parameter (Nix `{ a ? default }`)",
                ));
            }
            body = Expr::Lambda {
                param: self.param(&item.pattern)?,
                body: Box::new(body),
            };
        }
        Ok(body)
    }

    fn arrow_body(&mut self, arrow: &ast::ArrowFunctionExpression<'_>) -> Result<Expr, Error> {
        if arrow.expression {
            let Some(ast::Statement::ExpressionStatement(statement)) = arrow.body.statements.first()
            else {
                return Err(self.err(arrow.body.span, "malformed arrow expression body"));
            };
            return self.expr(&statement.expression);
        }

        let mut bindings = Vec::new();
        let mut result = None;
        for statement in &arrow.body.statements {
            if result.is_some() {
                return Err(self.err(statement.span(), "unreachable statement after `return`"));
            }
            match statement {
                ast::Statement::VariableDeclaration(declaration) => {
                    self.const_bindings(declaration, &mut bindings)?;
                }
                ast::Statement::ReturnStatement(ret) => {
                    let Some(argument) = &ret.argument else {
                        return Err(self.err(
                            ret.span,
                            "`return` must carry a value; Nix has no `undefined`",
                        ));
                    };
                    result = Some(self.expr(argument)?);
                }
                other => {
                    return Err(self.err(
                        other.span(),
                        "an arrow block body maps to `let ... in`: only `const` \
                         declarations and a final `return <expr>` are allowed",
                    ));
                }
            }
        }
        let Some(body) = result else {
            return Err(self.err(
                arrow.body.span,
                "arrow block body must end with `return <expr>`",
            ));
        };
        Ok(make_let(bindings, body))
    }

    fn param(&mut self, pattern: &ast::BindingPattern<'_>) -> Result<Param, Error> {
        match pattern {
            ast::BindingPattern::BindingIdentifier(ident) => Ok(Param::Ident(
                self.checked_name(ident.span, ident.name.as_str())?,
            )),
            ast::BindingPattern::ObjectPattern(object) => self.object_param(object),
            ast::BindingPattern::ArrayPattern(array) => Err(self.err(
                array.span,
                "array destructuring has no Nix equivalent; \
                 only object patterns map to attrset patterns",
            )),
            ast::BindingPattern::AssignmentPattern(assign) => Err(self.err(
                assign.span,
                "defaults are only expressible inside a destructured object \
                 parameter (Nix `{ a ? default }`)",
            )),
        }
    }

    fn object_param(&mut self, object: &ast::ObjectPattern<'_>) -> Result<Param, Error> {
        let mut fields: Vec<PatternField> = Vec::new();
        for property in &object.properties {
            let ast::PropertyKey::StaticIdentifier(key) = &property.key else {
                return Err(self.err(
                    property.span,
                    "attrset patterns bind plain identifier keys only",
                ));
            };
            let name = self.checked_name(key.span, key.name.as_str())?;
            if fields.iter().any(|field| field.name == name) {
                return Err(self.err(property.span, format!("duplicate pattern field `{name}`")));
            }

            let default = match &property.value {
                ast::BindingPattern::BindingIdentifier(value) if value.name == key.name => None,
                ast::BindingPattern::AssignmentPattern(assign) => {
                    let renamed = !matches!(
                        &assign.left,
                        ast::BindingPattern::BindingIdentifier(left)
                            if left.name == key.name
                    );
                    if renamed {
                        return Err(self.err(
                            property.span,
                            "renaming in a destructured parameter has no Nix equivalent",
                        ));
                    }
                    Some(self.expr(&assign.right)?)
                }
                _ => {
                    return Err(self.err(
                        property.span,
                        "renaming or nesting in a destructured parameter \
                         has no Nix equivalent",
                    ));
                }
            };
            fields.push(PatternField { name, default });
        }

        let bind = match &object.rest {
            None => None,
            Some(rest) => match &rest.argument {
                ast::BindingPattern::BindingIdentifier(ident) => {
                    Some(self.checked_name(ident.span, ident.name.as_str())?)
                }
                _ => {
                    return Err(
                        self.err(rest.span, "rest must bind a plain identifier (`...rest`)")
                    );
                }
            },
        };
        Ok(Param::Pattern {
            fields,
            ellipsis: object.rest.is_some(),
            bind,
        })
    }

    /// `f(a, b)` maps to curried application `f a b`.
    fn call(&mut self, call: &ast::CallExpression<'_>) -> Result<Expr, Error> {
        if call.optional {
            return Err(self.err(call.span, "optional calls have no Nix equivalent"));
        }
        if call.arguments.is_empty() {
            return Err(self.err(
                call.span,
                "Nix has no zero-argument functions; pass an explicit argument",
            ));
        }

        let mut mapped = self.expr(&call.callee)?;
        for argument in &call.arguments {
            let Some(expression) = argument.as_expression() else {
                return Err(self.err(
                    argument.span(),
                    "spread arguments have no Nix equivalent",
                ));
            };
            mapped = Expr::Apply {
                function: Box::new(mapped),
                argument: Box::new(self.expr(expression)?),
            };
        }
        Ok(mapped)
    }

    /// `import("./x.ix")` / `import("./x.nix")` map to `__importIx` / `import`
    /// applied to `__dir`-relative paths; the module wrapper follows.
    fn import(&mut self, import: &ast::ImportExpression<'_>) -> Result<Expr, Error> {
        if import.options.is_some() {
            return Err(self.err(import.span, "import options have no Nix equivalent"));
        }
        let ast::Expression::StringLiteral(specifier) = &import.source else {
            return Err(self.err(
                import.source.span(),
                "import specifier must be a string literal",
            ));
        };

        let spec = specifier.value.as_str();
        let relative = if let Some(rest) = spec.strip_prefix("./") {
            rest
        } else if spec.starts_with("../") {
            spec
        } else {
            return Err(self.err(
                specifier.span,
                "import specifier must be relative (`./` or `../`)",
            ));
        };
        let extension = std::path::Path::new(spec).extension();
        let function = if extension.is_some_and(|extension| extension == "ix") {
            "__importIx"
        } else if extension.is_some_and(|extension| extension == "nix") {
            "import"
        } else {
            return Err(self.err(
                specifier.span,
                "import specifier must name a `.ix` or `.nix` file",
            ));
        };

        self.uses_import = true;
        Ok(Expr::Apply {
            function: Box::new(Expr::Ident(function.into())),
            argument: Box::new(Expr::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(Expr::Ident("__dir".into())),
                rhs: Box::new(Expr::Str(vec![StrPart::Lit(format!("/{relative}"))])),
            }),
        })
    }

    fn static_member(&mut self, member: &ast::StaticMemberExpression<'_>) -> Result<Expr, Error> {
        let base = self.expr(&member.object)?;
        Ok(push_select(base, Attr::Name(member.property.name.to_string())))
    }

    fn computed_member(
        &mut self,
        member: &ast::ComputedMemberExpression<'_>,
    ) -> Result<Expr, Error> {
        let key = match &member.expression {
            ast::Expression::StringLiteral(lit) => Attr::Name(lit.value.to_string()),
            ast::Expression::NumericLiteral(lit) => {
                return Err(self.err(
                    lit.span,
                    "list indexing has no Nix operator; use `builtins.elemAt`",
                ));
            }
            other => Attr::Dynamic(self.expr(other)?),
        };
        let base = self.expr(&member.object)?;
        Ok(push_select(base, key))
    }

    fn logical(&mut self, logical: &ast::LogicalExpression<'_>) -> Result<Expr, Error> {
        let op = match logical.operator {
            ast::LogicalOperator::And => BinaryOp::And,
            ast::LogicalOperator::Or => BinaryOp::Or,
            ast::LogicalOperator::Coalesce => {
                return self.coalesce(logical);
            }
        };
        Ok(Expr::Binary {
            op,
            lhs: Box::new(self.expr(&logical.left)?),
            rhs: Box::new(self.expr(&logical.right)?),
        })
    }

    /// `x.y?.z ?? d` maps to `x.y.z or d`. `??` anywhere else has no 1:1.
    fn coalesce(&mut self, logical: &ast::LogicalExpression<'_>) -> Result<Expr, Error> {
        let ast::Expression::ChainExpression(chain) = &logical.left else {
            return Err(self.err(
                logical.span,
                "`??` maps to Nix `or` and needs an optional chain on its left \
                 (`x.y?.z ?? default`)",
            ));
        };
        let selected = match &chain.expression {
            ast::ChainElement::StaticMemberExpression(member) => self.static_member(member)?,
            ast::ChainElement::ComputedMemberExpression(member) => self.computed_member(member)?,
            other => {
                return Err(self.err(
                    other.span(),
                    "only attribute access can appear under `?.`",
                ));
            }
        };
        let Expr::Select {
            base,
            path,
            or_default: None,
        } = selected
        else {
            return Err(self.err(chain.span, "only attribute access can appear under `?.`"));
        };
        Ok(Expr::Select {
            base,
            path,
            or_default: Some(Box::new(self.expr(&logical.right)?)),
        })
    }

    fn binary(&mut self, binary: &ast::BinaryExpression<'_>) -> Result<Expr, Error> {
        use ast::BinaryOperator as JsOp;
        let op = match binary.operator {
            JsOp::Equality => BinaryOp::Eq,
            JsOp::Inequality => BinaryOp::Ne,
            JsOp::StrictEquality => {
                return Err(self.err(binary.span, "`===` has no Nix equivalent; use `==`"));
            }
            JsOp::StrictInequality => {
                return Err(self.err(binary.span, "`!==` has no Nix equivalent; use `!=`"));
            }
            JsOp::LessThan => BinaryOp::Lt,
            JsOp::LessEqualThan => BinaryOp::Le,
            JsOp::GreaterThan => BinaryOp::Gt,
            JsOp::GreaterEqualThan => BinaryOp::Ge,
            JsOp::Addition => BinaryOp::Add,
            JsOp::Subtraction => BinaryOp::Sub,
            JsOp::Multiplication => BinaryOp::Mul,
            JsOp::Division => BinaryOp::Div,
            other => {
                return Err(self.err(
                    binary.span,
                    format!("operator `{}` has no Nix equivalent", other.as_str()),
                ));
            }
        };
        Ok(Expr::Binary {
            op,
            lhs: Box::new(self.expr(&binary.left)?),
            rhs: Box::new(self.expr(&binary.right)?),
        })
    }

    fn unary(&mut self, unary: &ast::UnaryExpression<'_>) -> Result<Expr, Error> {
        let op = match unary.operator {
            ast::UnaryOperator::LogicalNot => UnaryOp::Not,
            ast::UnaryOperator::UnaryNegation => UnaryOp::Neg,
            other => {
                return Err(self.err(
                    unary.span,
                    format!("operator `{}` has no Nix equivalent", other.as_str()),
                ));
            }
        };
        Ok(Expr::Unary {
            op,
            operand: Box::new(self.expr(&unary.argument)?),
        })
    }

    /// Collects a `const` declaration into `let` bindings, rejecting `let` /
    /// `var` and duplicate names.
    fn const_bindings(
        &mut self,
        declaration: &ast::VariableDeclaration<'_>,
        bindings: &mut Vec<LetBinding>,
    ) -> Result<(), Error> {
        if declaration.kind != ast::VariableDeclarationKind::Const {
            return Err(self.err(
                declaration.span,
                "only `const` maps to a Nix `let` binding; `let` and `var` imply mutation",
            ));
        }
        for declarator in &declaration.declarations {
            let ast::BindingPattern::BindingIdentifier(ident) = &declarator.id else {
                return Err(self.err(
                    declarator.id.span(),
                    "destructuring `const` has no Nix equivalent; bind one name",
                ));
            };
            let name = self.checked_name(ident.span, ident.name.as_str())?;
            if bindings.iter().any(|binding| binding.name == name) {
                return Err(self.err(ident.span, format!("duplicate `const {name}`")));
            }
            let Some(init) = &declarator.init else {
                return Err(self.err(declarator.span, "`const` must have an initializer"));
            };
            bindings.push(LetBinding {
                name,
                value: self.expr(init)?,
            });
        }
        Ok(())
    }
}

/// Extends a select path (`x.y` + `.z` = `x.y.z`) instead of nesting selects,
/// so rendered attrpaths read like the source.
fn push_select(base: Expr, attr: Attr) -> Expr {
    match base {
        Expr::Select {
            base,
            mut path,
            or_default: None,
        } => {
            path.push(attr);
            Expr::Select {
                base,
                path,
                or_default: None,
            }
        }
        other => Expr::Select {
            base: Box::new(other),
            path: vec![attr],
            or_default: None,
        },
    }
}

/// Left-folds `operands` with `op`; a single operand stays bare.
fn fold_binary(op: BinaryOp, operands: Vec<Expr>) -> Expr {
    let mut operands = operands.into_iter();
    let first = operands
        .next()
        .expect("fold_binary callers always push at least one operand");
    operands.fold(first, |lhs, rhs| Expr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}
