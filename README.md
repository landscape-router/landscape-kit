# Landscape Kit

Landscape Kit provides the `lkit` terminal console and CLI for installing and operating a [Landscape](https://github.com/ThisSeanZhang/landscape) instance. It covers first-time deployment, migrations, updates, version switching, repair, backups, network takeover, and service lifecycle management.

The workspace also includes `lflare`, a separate client for the Landscape Terrain L2 recovery channel. Use it when the normal IP path to a router is unavailable.

## Quick Start

The current release binaries support glibc-based Linux on `x86_64` and `aarch64`. musl-based distributions such as Alpine are not supported. The installer needs `curl` or `wget`, plus `sudo` access:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

Or:

```sh
wget -qO- https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

Start the interactive management console from a terminal:

```sh
sudo lkit
```

The bare `lkit` command opens the Ratatui console. Automation should use an explicit subcommand and `--non-interactive`, for example:

```sh
sudo lkit --non-interactive check
sudo lkit --non-interactive install --password-file /root/lkit-password
```

To use a specific Landscape repository:

```sh
sudo lkit install --repository https://l1s3.whileaway.dev/landscape/
```

The interface follows the system locale and supports English and simplified Chinese. Override it with `--lang en`, `--lang zh`, or `LKIT_LANG`.

The installer verifies release assets against `SHA256SUMS` and atomically installs `lkit` at `/usr/local/bin/lkit`. See the [release and installation specification](docs/release/lkit.md) for supported targets, upgrade behavior, and security details.

## Common Commands

Use the command-specific documentation for all options, confirmation rules, and failure recovery.

| Area | Commands |
| --- | --- |
| Inspect and install | [`check`](docs/check.md), [`install`](docs/commands/install.md), [`migrate`](docs/commands/migrate.md) |
| Versions and repair | [`update`](docs/commands/update.md), [`switch`](docs/commands/switch.md), [`repair`](docs/commands/repair.md), [`reinit`](docs/commands/reinit.md) |
| Backups and state | [`backup`](docs/commands/backup.md), [`restore`](docs/commands/restore.md), [`reconcile`](docs/commands/reconcile.md) |
| Network and host setup | [`network`](docs/commands/network.md), [`set-mirror`](docs/commands/mirror.md), [`software`](docs/commands/software.md) |
| Removal and lkit service | [`uninstall`](docs/commands/uninstall.md), [`self`](docs/commands/self.md) |

## Terrain Recovery Channel

Terrain is an encrypted Layer 2 recovery path between a host and a Landscape router. The `lkit` daemon can host the server side; configure or inspect its recovery secret with `lkit flare setup`.

`lflare` opens an interactive client by default. Scripts can use its `cli` subcommand:

```sh
lflare cli --psk '<recovery-secret>' --dev eth0 --forward 2222:22
```

Linux clients require a supported glibc target. Windows clients require Npcap. Read the [Terrain documentation](docs/flare/README.md) for protocol details, configuration, and end-to-end scenarios.

## Workspace

| Crate | Role |
| --- | --- |
| `lkit-cli` | The `lkit` binary: console, commands, workflows, and daemon |
| `landscape-flare` | The `lflare` Terrain recovery client |
| `landscape-terrain-proto` | Terrain L2 protocol and transport library |
| `crates/lkit-hostnet` | Host network adaptation and rollback library |
| `crates/lkit-publish` | `lkit-publish`, the release repository publisher |
| `crates/lkit-repository` | Repository protocol types shared by the CLI and publisher |
| `crates/lkit-test-fixture` | Isolated fixture binaries used by tests; not a runtime dependency |

## Building and Testing

Build the complete workspace:

```sh
cargo build --locked --workspace
```

For a focused local check, run formatting, Clippy, and the unit tests for the module you changed:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test -p lkit-cli --features test-support --bin lkit <module-filter>
```

The test suite is layered. Docker, systemd, QEMU, publishing, and Terrain scenarios have dedicated environments and CI workflows; see the [testing guide](docs/testing/README.md) before running them locally.

## Documentation and Contributing

- [Documentation index](docs/README.md)
- [Release and installation](docs/release/lkit.md)
- [Testing guide](docs/testing/README.md)
- [Contributing guide](CONTRIBUTING.md) · [中文贡献指南](CONTRIBUTING.zh.md)

Issues are optional. For code changes, follow the repository contribution and testing workflow described in the contributing guide.
