# Landscape Kit

`lkit` is a command-line tool for managing a [Landscape](https://landscape.canonical.com/) instance: first-time installation, version switching, repair, state reconciliation, and service manager migration.

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
- `repair` — repair static pages or the backend binary.
- `reconcile` — accept and record changes to init files, service units, or repository sources.
- `service-manager` — migrate between systemd and external process management.

## Documentation

Specifications and design documents live in [`docs/`](docs/README.md). A Chinese-language version of this readme is available at [README.zh.md](README.zh.md).

## Installing Landscape

Install the latest `lkit` release and start the interactive Landscape installation on a supported
Debian x86_64 or aarch64 host:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh -s -- install
```

The installer verifies the selected binary against the release `SHA256SUMS` before atomically
installing it at `/usr/local/bin/lkit`. See the [lkit release specification](docs/release/lkit.md)
for the release assets, version policy, and manual publishing procedure.

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
