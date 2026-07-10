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
    use super::tool_server_info;

    #[test]
    fn tool_server_metadata_has_identity_instructions_and_capability() {
        let info = tool_server_info("example", "1.2.3", "Do the thing");

        assert_eq!(info.server_info.name, "example");
        assert_eq!(info.server_info.version, "1.2.3");
        assert_eq!(info.instructions.as_deref(), Some("Do the thing"));
        assert!(info.capabilities.tools.is_some());
    }
}
