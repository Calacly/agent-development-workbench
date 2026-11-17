# /// script
# dependencies = ["mcp>=1.20,<2"]
# ///
"""Minimal MCP server on a pre-2026-07-28 revision (initialize handshake) for interop tests."""
from mcp.server.fastmcp import FastMCP

server = FastMCP("echo-legacy")


@server.tool()
def echo(text: str) -> str:
    """Return the input text."""
    return text


if __name__ == "__main__":
    server.run()
