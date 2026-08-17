# AGENTS

## Contribution Workflow

- Issues are optional; code changes and PRs do not require a linked issue.
- You may ask the user whether they want to create an issue for the change
  before starting work.

## Commit Convention

Write commit messages in English.

Before each commit, run only unit tests and static checks (never the e2e fixture
suite or the full test suite):

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test -p lkit-cli <module-filter>
```

## Testing

- Run unit tests for the current change only (e.g. `cargo test -p lkit-cli <module-filter>`),
  never the full test suite after every code change.
- The e2e fixture suite (`lkit-cli/tests/install_fixture_e2e.rs`, ~6 minutes) runs in CI
  on every push and as a PR check via `.github/workflows/test-fixture-e2e.yml`, not locally before each
  commit. To run it manually: `cargo test -p lkit-cli --features test-support --test install_fixture_e2e`.

### Testing Hygiene

Unit tests must be free of real system side effects. Everything that cannot run in an
isolated temporary directory belongs in the e2e fixture suite (CI) or the container-based
scripts under `scripts/`, never in unit tests.

- Unit tests may only touch temporary directories; lkit's fixed territory `/root/.lkit/` is
  reachable only through `deployment::layout`'s `LKIT_TERRITORY` env override
  (see `test_territory()` in `lkit-cli/src/deployment/layout.rs`). Never write to
  `/root/.lkit`, `/etc/systemd`, `/usr/local`, or any real host path in a unit test.
- Never spawn real processes (`lkit daemon`, `landscape-webserver`, `systemctl`, ...), bind
  ports, or drive real systemd/network state from unit tests. Use the fake managers and
  fixtures the codebase already provides.
- System-level scenarios that cannot be isolated (real daemon deployment, network takeover,
  service manager backends) belong in `lkit-cli/tests/install_fixture_e2e/`, which
  runs only in CI or containers: every test there starts with an `e2e_enabled()` gate and
  requires the `LKIT_E2E=1` environment variable to actually run.
- When verifying locally, always scope unit tests to the `lkit` binary and an exact module:
  `cargo test -p lkit-cli --features test-support --bin lkit <module-filter>`. Do not use
  bare `cargo test -p lkit-cli --features test-support <filter>` and do not use substring
  filters like `daemon::` that also match e2e fixture test names — running the fixture
  suite on a host without a container can hang and leak daemon/webserver processes.
- Never run the e2e fixture suite (`--test install_fixture_e2e`) or the container scripts
  locally as part of normal development; CI owns them.

## Project Layout

- `lkit-cli/` — the `lkit` binary crate (the shipped executable); hosts the flare server
  (`src/flare/`, wired as the `lkit flare serve|sniff` subcommand and the daemon `[flare]`
  config section).
- `landscape-terrain-proto/` — the Terrain L2 protocol library (publishable crate, own
  version line; wire magic `TERR`, crypto labels `terrain-*`).
- `landscape-flare/` — the L2 client crate; binary `lflare` / `lflare.exe` (Linux
  AF_PACKET, Windows libpcap; `vendor/npcap-sdk/` is tracked).
- `crates/` — internal Cargo workspace library members: `lkit-hostnet`, `lkit-publish`, `lkit-repository`, `lkit-test-fixture`.
- `docs/` — specifications and design documents at the repository root (moved from `lkit-cli/docs`).
- `scripts/` — integration test scripts (flare e2e: `scripts/flare/e2e-*.sh` +
  `scripts/flare/Dockerfile`, see `docs/flare/`).

## Documentation

- Behavior changes must be reflected in `docs/`.
- There is no language requirement for new documents.

### Test Scenario Documentation

- Store the scenario catalog index at `docs/testing/scenarios/README.md`.
- Store core functional scenarios in `docs/testing/scenarios/functional/`, using one Markdown file
  per domain, such as `publish.md` or `install.md`.
- Store real-systemd compatibility scenarios in `docs/testing/scenarios/systemd-smoke.md`.
- Write each domain document in this format:

```md
# <Domain title>

## <SCENARIO-ID>

**<Scenario title>**

- 测试层：<layer>
- 状态：`<status>`
- 证据：<links to specifications, scripts, or tests>
- 缺口：<optional missing coverage>
- 说明：<optional additional context>
```

- Use stable uppercase IDs such as `PUB-01`, `INS-01`, and `RB-01`, and list each domain document
  with its ID range in the catalog index.
