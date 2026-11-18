# MCP Client

Web app to connect MCP servers and LLM providers, test tools and chat. One Rust crate; the binary serves the REST API and the web UI.

- Run: `cargo run` (port 3000, `PORT` overrides; listens on 127.0.0.1, `HOST` overrides; database `./mcp_client.db`, `DATABASE_URL` overrides)
- Stack: axum, sqlx (SQLite, `migrations/`), rmcp for MCP, askama templates in `templates/`, htmx and CSS/JS in `static/` (embedded into the binary)
- Code: `src/routes/` REST API, `src/web/` HTML pages, `src/services/` LLM and MCP clients, `src/container.rs` sandboxed stdio servers (see docs/sandbox.md)
- Tests: `cargo test` (each test file starts the app with a temporary database). `cargo test -- --ignored` runs the tests that need `uv` and network.
- Write tests for every new or changed API endpoint.
