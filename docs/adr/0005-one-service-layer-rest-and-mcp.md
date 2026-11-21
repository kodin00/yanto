# One service layer behind the dashboard, REST, and MCP, with unified API Tokens

All operations live in one service layer. The htmx dashboard, the `/api/v1` REST API, and MCP (streamable HTTP plus `yanto mcp` over stdio, via the `rmcp` SDK) are thin adapters over it. A single API Token type with `read` < `write` < `admin` scopes authenticates both REST and MCP, replacing the old separate `ymcp_` tokens. MCP is kept as an interface onto devops operations, not as an AI feature. If `rmcp` proves unstable, MCP over HTTP is dropped first and stdio stays.
