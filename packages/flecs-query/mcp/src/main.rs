//! `ix-flecs-query-mcp`: a stdio MCP server over [`flecs_query_core`].
//!
//! Three thin tools: `parse` returns the typed AST as JSON, `canonicalize`
//! returns the normalized expression text, and `validate` returns a
//! non-erroring verdict for linting flows. All language behavior lives in
//! the core crate; this binary only shapes arguments and errors.

use rmcp::ServiceExt as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ErrorCode, ErrorData, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = FlecsQueryMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// The MCP server. Stateless: every tool parses its argument afresh.
#[derive(Clone)]
struct FlecsQueryMcp {
    tool_router: ToolRouter<Self>,
}

/// The one argument every tool takes.
#[derive(Deserialize, JsonSchema)]
struct ExprArgs {
    /// A Flecs Query Language expression, e.g.
    /// `Position, [in] Velocity, (ChildOf, $parent)`.
    expr: String,
}

/// The verdict returned by `validate`.
#[derive(Serialize)]
struct Verdict {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// The error rendered with a caret pointing into the expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    rendered: Option<String>,
}

impl FlecsQueryMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
#[allow(
    clippy::unused_self,
    reason = "rmcp tool methods must take &self for the ToolRouter even though this server is stateless"
)]
impl FlecsQueryMcp {
    #[tool(
        description = "Parse a Flecs Query Language expression \
                       (https://www.flecs.dev, e.g. 'Position, [in] Velocity, \
                       (ChildOf, $parent)') into its typed AST as JSON. \
                       Errors carry a byte span and a caret-rendered message."
    )]
    fn parse(&self, Parameters(args): Parameters<ExprArgs>) -> Result<String, ErrorData> {
        let query = parse_expr(&args.expr)?;
        serde_json::to_string(&query).map_err(|err| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("serializing the AST: {err}"),
                None,
            )
        })
    }

    #[tool(
        description = "Parse a Flecs Query Language expression and return its \
                       canonical form: normalized whitespace, comments dropped, \
                       implicit-source pairs in '(Rel, Tgt)' shape."
    )]
    fn canonicalize(&self, Parameters(args): Parameters<ExprArgs>) -> Result<String, ErrorData> {
        Ok(parse_expr(&args.expr)?.to_string())
    }

    #[tool(
        description = "Check whether a string is well-formed Flecs Query \
                       Language. Never errors: returns {valid, error?, \
                       rendered?} so it can be used in linting loops."
    )]
    fn validate(&self, Parameters(args): Parameters<ExprArgs>) -> Result<String, ErrorData> {
        let verdict = match flecs_query_core::parse(&args.expr) {
            Ok(_) => Verdict {
                valid: true,
                error: None,
                rendered: None,
            },
            Err(error) => Verdict {
                valid: false,
                rendered: Some(error.render(&args.expr)),
                error: Some(error.to_string()),
            },
        };
        serde_json::to_string(&verdict).map_err(|err| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("serializing the verdict: {err}"),
                None,
            )
        })
    }
}

fn parse_expr(expr: &str) -> Result<flecs_query_core::Query, ErrorData> {
    flecs_query_core::parse(expr).map_err(|error| {
        ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            error.render(expr),
            serde_json::to_value(&error).ok(),
        )
    })
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FlecsQueryMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo and Implementation are #[non_exhaustive]: start from a
        // Default and patch the fields we care about.
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        "ix-flecs-query-mcp".clone_into(&mut info.server_info.name);
        env!("CARGO_PKG_VERSION").clone_into(&mut info.server_info.version);
        info.instructions = Some(
            "Parse, canonicalize, and validate Flecs Query Language \
             expressions. Parsing is world-independent: it checks form, not \
             whether identifiers resolve in any particular flecs world."
                .to_owned(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;

    use super::parse_expr;

    #[test]
    fn parse_errors_are_invalid_params_with_a_caret() {
        let err = parse_expr("Position,, Velocity").expect_err("rejects");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains('^'), "got: {}", err.message);
    }

    #[test]
    fn parse_returns_the_query() {
        let query = parse_expr("Position, (ChildOf, $parent)").expect("parses");
        assert_eq!(query.terms.len(), 2);
    }
}
