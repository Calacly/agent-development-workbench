# /// script
# dependencies = ["mcp>=2,<3"]
# ///
"""Minimal MCP server on the 2026-07-28 revision (MCP v2) for interop tests."""
from mcp.server.mcpserver import MCPServer

server = MCPServer("echo-v2")


@server.tool()
def echo(text: str) -> str:
    """Return the input text."""
    return text


if __name__ == "__main__":
    server.run()
