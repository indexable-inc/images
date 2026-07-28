//! Shared metadata construction for tool-only [`rmcp`] servers.

use rmcp::model::{ServerCapabilities, ServerInfo};

/// Static identity and usage instructions for a tool-only MCP server.
pub struct ToolServer {
    /// Protocol-visible implementation name.
    pub name: &'static str,
    /// Agent-facing instructions advertised during initialization.
    pub instructions: &'static str,
}

impl ToolServer {
    /// Materialize rmcp's non-exhaustive metadata with the package version.
    #[must_use]
    pub fn info(&self, version: &str) -> ServerInfo {
        tool_server_info(self.name, version, self.instructions)
    }

    /// Metadata for a server that reports its own readiness, with the
    /// instructions computed at connect time rather than fixed at compile
    /// time.
    ///
    /// A server that can be unhealthy has something to say that a `&'static
    /// str` cannot express -- which account is signed in, which capability
    /// is failing right now -- and `instructions` is the channel that
    /// reliably reaches the agent, because hosts inject it into the model's
    /// context.
    ///
    /// The human is reached separately, through stderr. MCP's protocol
    /// logging (`notifications/message`) would have been the obvious second
    /// channel, but SEP-2577 deprecated it as of protocol version
    /// 2026-07-28: new implementations are told not to adopt it and to use
    /// stderr on stdio transports instead. Nothing in-protocol replaces it.
    #[must_use]
    pub fn live_info(&self, version: &str, instructions: &str) -> ServerInfo {
        tool_server_info(self.name, version, instructions)
    }
}

/// Build the standard metadata exposed by an ix MCP server that only provides
/// tools. Starting from `Default` is required because rmcp's metadata structs
/// are non-exhaustive.
#[must_use]
pub fn tool_server_info(name: &str, version: &str, instructions: &str) -> ServerInfo {
    let mut info = ServerInfo::default();
    info.capabilities = ServerCapabilities::builder().enable_tools().build();
    name.clone_into(&mut info.server_info.name);
    version.clone_into(&mut info.server_info.version);
    info.instructions = Some(instructions.to_owned());
    info
}

#[cfg(test)]
mod tests {
    use super::{ToolServer, tool_server_info};

    #[test]
    fn tool_server_metadata_has_identity_instructions_and_capability() {
        let info = tool_server_info("example", "1.2.3", "Do the thing");

        assert_eq!(info.server_info.name, "example");
        assert_eq!(info.server_info.version, "1.2.3");
        assert_eq!(info.instructions.as_deref(), Some("Do the thing"));
        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn live_metadata_carries_runtime_instructions() {
        let server = ToolServer {
            name: "example",
            instructions: "compile-time text",
        };

        let info = server.live_info("1.2.3", "Gmail is not signed in");

        assert_eq!(
            info.instructions.as_deref(),
            Some("Gmail is not signed in"),
            "runtime instructions must win over the static field"
        );
        assert!(info.capabilities.tools.is_some(), "tools stay declared");
    }

    #[test]
    fn no_server_declares_the_deprecated_logging_capability() {
        let server = ToolServer {
            name: "example",
            instructions: "text",
        };

        // SEP-2577 deprecated protocol logging as of 2026-07-28 and tells new
        // implementations not to adopt it. This asserts we did not, so
        // reintroducing it is a test failure rather than a quiet regression.
        assert!(server.info("1.0.0").capabilities.logging.is_none());
        assert!(server.live_info("1.0.0", "text").capabilities.logging.is_none());
    }
}
