# Upstream baseline

## Provenance

- Upstream repository: <https://github.com/choas/mcp-client>
- Baseline revision: `ba151cec986d55ff375d143e5a963d32fd0d543c`
- Baseline revision date: 2026-11-19
- Upstream project version: `1.0.1`
- License: MIT
- Upstream copyright: 2025-2026 Lars Gregori

The upstream `LICENSE` file and attribution must remain with copies or
substantial portions of the original software.

## Test baseline

The unmodified baseline was tested with:

- `rustc 1.100.0`
- `cargo 1.100.0`
- `rmcp 3.10.0`
- `cargo test --locked`

Result: 121 tests passed, none failed, and eight explicitly ignored tests were
not run. The ignored tests require `uv` and network access and cover MCP
protocol interoperability and model-to-tool integration.

## Foundation decision

The upstream application is adopted conditionally as:

- the Rust/Axum application shell;
- the MCP connection layer;
- the authenticated SQLite-backed web application;
- an initial execution-timeline presentation seam.

It is not adopted as the final observability core. Before live model, MCP,
retrieval, harness, memory, or application events are persisted, the workbench
must provide:

1. A versioned, provider-neutral event envelope.
2. Correlation across run, step, attempt, and parent operation.
3. A mandatory redaction or rejection boundary before persistence and export.
4. A distinction between observed events and inferred educational
   explanations.
5. Automated validation of event shape, ordering, correlation, and redaction.

The existing application can persist complete messages, tool arguments, tool
results, and timeline previews, and can write model and MCP request/response
logs. Those paths must not become the workbench trace store without the
sanitization boundary above.

## First implementation slice

Completed on `feature/synthetic-trace-workbench`. The slice uses explicitly
synthetic fixtures for one successful grounded-answer path and one retrieval
timeout. Both replay the same correlated event data in:

- a debug view with structured data, timing, status, and errors;
- a learn view with concise explanations grounded in selected events.

The versioned `1.0` envelope correlates run, step, attempt, operation, and
parent operation across harness, model, MCP, retrieval, memory, and
application components. Sensitive keys and secret-shaped values are redacted
before event construction completes, and trace validation rejects unsafe
mutation before fixture data is returned.

The authenticated `/workbench` page renders the fixtures as an offline D3
timeline. `/workbench/data` exposes only validated synthetic data. The
complete locked suite passes with 127 tests and eight existing ignored
network/`uv` tests. The page and data endpoint were also exercised through
the real registration and session-cookie flow.

Live provider, MCP, retrieval, and harness instrumentation remains a later
slice.

## Remote safety

The trainer repository is configured locally as fetch-only `upstream` with an
invalid push URL. The user-owned fork is
<https://github.com/Calacly/agent-development-workbench> and is configured as
the fetch and push `origin`. Git uses `origin` as the default push remote.
