# Landscape Kit

English | [中文](README.md)

[Landscape](https://github.com/ThisSeanZhang/landscape) is a Linux router system built with Rust + eBPF, featuring a web-based management interface and network configuration capabilities. **Landscape Kit (`lkit`)** is its companion CLI — it runs on the router host and handles installation, day-to-day management, diagnostics, and future upgrades.

Use it for first-time deployment, offline management when the web UI is unavailable, and automated/batch provisioning.

---

## Features

### Installation

```bash
# Interactive install — walks you through network config, source selection, download, and init
sudo lkit install

# Non-interactive install — for scripted or batch deployments
sudo lkit install --source github-default --version v0.19.2
```

Automatically detects system architecture and libc type, probes multiple sources (GitHub / HTTP mirrors / S3 / local) to pick the fastest, verifies SHA-256 checksums, and installs as a systemd service.

### Day-to-day Management

```bash
lkit status              # Service status
lkit service restart     # Restart service
lkit logs -n 100         # Last 100 log lines
lkit diagnose            # Health check (disk, API, systemd, ports, etc.)
```

Run `lkit` with no arguments to enter the interactive menu where every operation is a selection away.

### Mirror Management

```bash
# Sync artifacts from GitHub or HTTP mirrors to local disk
lkit mirror sync --target local --path /data/mirror --latest 5

# Sync to S3/R2 storage
lkit mirror sync --target s3 --bucket my-bucket --endpoint https://s3.example.com

# Verify a synced mirror
lkit mirror verify --target local --path /data/mirror

# Serve a local mirror over HTTP
lkit mirror serve --path /data/mirror --port 8080
```

Mirror management is useful for setting up private artifact sources in air-gapped networks or serving local downloads to multiple routers.

### Command Reference

| Command | Description |
|---------|-------------|
| `lkit` | Interactive main menu |
| `lkit status [--json]` | Service status |
| `lkit service start\|stop\|restart` | Service control |
| `lkit logs [-n N]` | Log viewer |
| `lkit diagnose [--json]` | System diagnostics |
| `lkit install` | Install / initialize Landscape |
| `lkit mirror sync\|serve\|verify\|list` | Release artifact mirror management |
| `lkit self version` | lkit version info |
| `lkit backup` | Backup management (planned) |
| `lkit upgrade` | Upgrade (planned) |
| `lkit rollback` | Rollback (planned) |

## Configuration

### Paths

| Item | Default | Override |
|------|---------|--------|
| Landscape data directory | `~/.landscape-router` | `LANDSCAPE_HOME` env var |
| lkit config directory | `~/.landscape-kit/` | `LKIT_HOME` env var |
| Custom artifact sources | `~/.landscape-kit/config/lkit.toml` | Edit the file |

### Custom Artifact Sources

Add extra sources in `lkit.toml`. During install, lkit probes all available sources automatically and picks the best one:

```toml
[[sources]]
name = "internal-mirror"
type = "http"
url = "https://mirror.example.com/landscape"
priority = 1

[[sources]]
name = "private-s3"
type = "s3"
bucket = "landscape-releases"
endpoint = "https://s3.example.com"
region = "us-east-1"
priority = 5
```

### Log Levels

```bash
lkit -v status      # INFO
lkit -vv status     # DEBUG
RUST_LOG=debug lkit # via environment variable
```

## Development

```bash
# Verify before committing
cargo fmt --all && cargo clippy --all -- -D warnings && cargo test --workspace
```

- Contribution guide: [CONTRIBUTING.md](CONTRIBUTING.md)
- Coding conventions: [docs/CONVENTIONS.md](docs/CONVENTIONS.md)
- Design specs: [docs/spec/](docs/spec/)

## Roadmap

| Milestone | Description | Status |
|-----------|-------------|--------|
| M1 | CLI skeleton, interactive menu, service management, logs, diagnostics | Done |
| M2 | Installation (wizard + systemd + network config) | Done |
| M2.5 | Multi-source download, mirror management tool | Done |
| M3 | Backup/restore, upgrade/rollback | Planned |

## License

[AGPL-3.0](LICENSE)
