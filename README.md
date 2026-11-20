# MCP Client

A multi-user web application for interacting with AI via the Model Context Protocol (MCP).

This repository is the starting point for the Agent Development Workbench.
The upstream provenance, adoption constraints, and clean test baseline are
recorded in [`docs/upstream-baseline.md`](docs/upstream-baseline.md).

## Features

- Multi-user authentication system
- Support for all LLM providers (configurable)
- Local (stdio) and remote (WebSocket/HTTP) MCP server support
- Server management UI
- Tool discovery and invocation
- Chat interface with LLM integration
- Resource and prompt browsing
- Comprehensive logging and debugging
- Synthetic agent-run workbench with correlated debug and learn views

## Technology Stack

- **Backend**: Rust with Axum framework
- **APIs**: REST + WebSocket
- **Frontend**: server-rendered HTML (askama templates, htmx, plain CSS/JS), built into the same binary as the API
- **Database**: SQLite
- **MCP**: rmcp (official Rust SDK)

## Project Structure

```
.
├── src/
│   ├── main.rs          # Application entry point
│   ├── config.rs        # Configuration management
│   ├── models/          # Data models
│   ├── routes/          # JSON API (/api/v1) and the chat WebSocket
│   ├── services/        # LLM and MCP clients
│   └── web/             # Web UI: page handlers, themes, view models
├── templates/           # HTML templates (askama, checked at compile time)
├── static/              # CSS, JavaScript, vendored htmx (embedded in the binary)
├── migrations/          # Database migrations
├── tests/               # API and web tests (start the app themselves)
├── sandbox/             # Sandboxed check of the modern-web-guidance CLI
├── docs/sandbox.md      # Local MCP servers: process and container mode
└── CLAUDE.md            # Project instructions
```

## Run (development)

Prerequisites: Rust (latest stable). No Node.js or bun needed.

```bash
cp .env.example .env      # then set JWT_SECRET and ENCRYPTION_KEY
cargo run
```

Open `http://localhost:3000` (or the `PORT` from `.env`). The first start
creates the SQLite database and runs the migrations.

The app listens on `127.0.0.1` only. Set `HOST=0.0.0.0` in `.env` to reach it from other machines.

- HTML templates are compiled into the binary: after changing one, restart `cargo run`.
- CSS and JavaScript in `static/` are read from disk in debug builds: reload the page.
- For containerized MCP servers behind a VPN, see `CONTAINER_DNS` in [docs/sandbox.md](docs/sandbox.md).

## Development

### Database Migrations

Migrations are automatically run on startup. Migration files are in `migrations/`.

### API Documentation

The REST API is available at `/api/v1`:

- **Authentication**: `/api/v1/auth/*`
- **LLM Providers**: `/api/v1/llm-providers/*`
- **MCP Servers**: `/api/v1/mcp-servers/*`
- **Conversations**: `/api/v1/conversations/*`

## Testing

Each test file starts the app itself, on a free port with its own SQLite database in the temp directory:

```bash
cargo test
cargo test -- --ignored           # tests that need uv and network access
```

## License

MIT, see [LICENSE](LICENSE).

`static/vendor/htmx-2.0.11.min.js` is htmx, licensed under the Zero-Clause BSD license.

`static/vendor/d3-7.9.0.min.js` is D3, licensed under the ISC license. Its
license is preserved in `static/vendor/d3-7.9.0.LICENSE`.
