# Landscape Kit

`lkit` is an interactive terminal console and command-line tool for managing a [Landscape](https://landscape.canonical.com/) instance: first-time installation, version switching, repair, state reconciliation, and service manager migration.

The repository is a Cargo workspace made up of four crates:

| Crate | Role |
| --- | --- |
| `crates/lkit-cli` | The `lkit` binary: commands, domain logic, and workflows |
| `crates/lkit-publish` | The `lkit-publish` binary: packs releases and publishes them to a repository |
| `crates/lkit-repository` | Repository protocol library shared by the CLI and the publisher |
| `crates/lkit-test-fixture` | Test fixtures: simulated `systemctl`, an HTTPS webserver, and a test repository |

## Commands

- `check` — host environment checks.
- `install` — first-time installation.
- `switch` — switch to a specified stable version.
- `backup` — create, inspect, and verify `.lkb` minimal backups.
- `restore` — restore an existing installation from an `.lkb` backup.
- `repair` — repair static pages or the backend binary.
- `reconcile` — accept and record changes to init files, service units, or repository sources.
- `service-manager` — migrate between systemd and external process management.

## Documentation

Specifications and design documents live in [`docs/`](docs/README.md). A Chinese-language version of this readme is available at [README.zh.md](README.zh.md).

## Installing Landscape

On a glibc-based Linux x86_64 or aarch64 host, install `lkit` first and then start the interactive
Landscape installation directly from the terminal:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

Or with `wget`:

```sh
wget -qO- https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

The installer itself auto-selects `curl` (preferred) or `wget` to download the release assets, so
either tool on the host is sufficient.

Then start the interactive installer:

```sh
sudo lkit
```

The bare command opens the Ratatui management console. Scripts and CI should use explicit
subcommands such as `lkit --non-interactive install ...`.

The interface follows the system locale and supports English and simplified Chinese. Use
`lkit --lang zh ...` or set `LKIT_LANG=zh` to override it; unsupported languages fall back to
English.

The installer verifies the selected binary against the release `SHA256SUMS` before atomically
installing it at `/usr/local/bin/lkit`. Distribution names are not allowlisted; `lkit` checks the
kernel and required host capabilities before deployment. Current release binaries do not support
musl-based distributions such as Alpine. See the [lkit release specification](docs/release/lkit.md)
for details.

## Building and Testing

```sh
cargo build --locked
cargo test --features test-support
```

Tests that depend on the fixture binaries require the `test-support` feature. The RustFS publishing integration test is not part of `cargo test`; run it separately:

```sh
RUSTFS_IMAGE=<pinned-image> scripts/test-publish-http-repository.sh
```
The Docker functional E2E runs locally on Linux x86_64; native aarch64 coverage runs in CI:

```sh
scripts/test-docker-lifecycle.sh
```

See [`docs/testing/README.md`](docs/testing/README.md) for the test layers, including the
low-frequency/manual real-systemd nspawn compatibility smoke test.
