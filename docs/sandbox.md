# Running local MCP servers

Local (stdio) MCP servers run either as a process on the host or in a container. Each server has a "Run in a container" option in the add/edit dialog.

## As a process (default)

- Code: `src/sandbox.rs`, `SandboxConfig::default()`
- The server inherits the host environment, plus `UV_NO_SYNC=1` and the server's own environment variables.
- No limits are applied: the server can read and write every file the user can, open network connections and start processes.
- rlimits are off because they break `uvx` on macOS: `RLIMIT_AS` counts mapped files and shared libraries, and forking then fails with `EAGAIN`.
- `SandboxConfig::strict()` (rlimits for memory, CPU time, file descriptors) exists in the code, but the app does not use it yet. A per-server setting is a TODO in `src/routes/mcp_servers.rs`.

Use this only for servers you trust.

## In a container

- Code: `src/container.rs`, `build_container_command` in `src/services/mcp_client.rs`
- Runtime: Podman if installed, otherwise Docker.
- The runtime pulls a missing image on the first start. A start may take up to 180 seconds (image pull plus package download).

Every container runs with:

```
--rm -i --cap-drop=ALL --security-opt=no-new-privileges --cpus=1.0 --tmpfs=/tmp
```

The rest depends on the server's command:

| Command | Image | Network | Root filesystem | Memory | pids limit |
|---|---|---|---|---|---|
| `uvx <package>` | `ghcr.io/astral-sh/uv:python3.15-trixie-slim` | bridge | writable | 1024 MB | 256 |
| `npx <package>`, `npm`, `node` | `node:26-slim` | bridge | writable | 1024 MB | 256 |
| anything else | `ghcr.io/astral-sh/uv:python3.15-trixie-slim` | none | read-only | 512 MB | 50 |

- uvx and npx need the network to download the server package from PyPI or npm.
- The pids limit counts threads too. uv, npm and node start many, and with 50 `uvx` fails with "Resource temporarily unavailable".
- The default images are Debian slim (glibc), not Alpine (musl). See DNS below.

### Custom image and parameters

- "Custom image" replaces only the image. Network, memory and the other limits still follow the command (table above).
- "Container parameters" are added to `run` before the security defaults, for example:

```bash
-v /my/data:/data:ro          # mount a host directory read-only
-v /data:/data:ro -v /output:/output --env MY_VAR=value
```

Parameters such as `--cap-add` or `--network=host` weaken the isolation.

### Pre-built image without network

For a server that should run with `--network=none`, put the package into the image:

```dockerfile
FROM ghcr.io/astral-sh/uv:python3.15-trixie-slim
RUN uvx --no-cache mcp-server-git --help || true
WORKDIR /workspace
ENTRYPOINT ["uvx", "--no-cache"]
CMD ["mcp-server-git"]
```

Build it with `podman build -t mcp-server-git:latest .` and select it as "Custom image". Note that the command decides the network mode: with command `uvx` the container still gets `bridge`.

### DNS (Podman machine on macOS, VPN)

Podman machine's DNS forwarder can answer IPv6 (AAAA) lookups with SERVFAIL. glibc ignores that, musl gives up with `EAI_AGAIN`. This affects Alpine images and the `uv`/`uvx` binary (built for musl even in the Debian image), so `uvx` cannot reach PyPI. Fix: set DNS servers in `.env`:

```bash
CONTAINER_DNS=192.0.2.53         # e.g. your network's or VPN's DNS server (example address)
# CONTAINER_DNS=1.1.1.1,8.8.8.8  # or public resolvers, comma-separated
```

The app adds `--dns=<server>` to every container that has network access. Check with:

```bash
podman run --rm --dns=<server> ghcr.io/astral-sh/uv:python3.15-trixie-slim uvx --quiet pycowsay hi
```

### Updating images

The runtime keeps pulled images. To get a newer version:

```bash
podman pull ghcr.io/astral-sh/uv:python3.15-trixie-slim
podman pull node:26-slim
```

## Not covered

- Process mode: no filesystem, network or syscall restrictions.
- Container mode: no syscall filtering beyond the runtime's default seccomp profile, and bridge networking gives full outbound access.
- For untrusted servers: use a pre-built image with `--network=none`, mount only what the server needs, or run it in a VM.
