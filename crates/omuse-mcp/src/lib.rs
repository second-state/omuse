//! MCP connectors (`app/sentinel/{mcp_client,mcp_oauth,mcp_hub}.py`): the JSON-RPC client over Streamable HTTP
//! and legacy SSE, OAuth 2.1 for MCP servers (tokens in the vault), and the hub that keeps the user's servers in
//! the store and pins their tool definitions into `omuse_sentinel_core::catalog::TOOLS` as `mcp_<server>__<tool>`.

pub mod mcp_client;
pub mod mcp_hub;
pub mod mcp_oauth;

pub use mcp_client::{check_server_url, connect, Client, Headers, MCPError};
pub use mcp_hub::HubError;
pub use mcp_oauth::OAuthError;
