# AGENTS

## Commit Convention

Write commit messages in English.

Run formatting and build before committing:

```sh
cargo fmt
```

## Testing

- Run unit tests for the current change only (e.g. `cargo test -p lkit-cli <module-filter>`),
  never the full test suite after every code change.
- The e2e fixture suite (`crates/lkit-cli/tests/install_fixture_e2e.rs`, ~6 minutes) runs only
  as a PR check via `.github/workflows/test-e2e.yml`, not locally after each change. To run it
  manually: `cargo test -p lkit-cli --features test-support --test install_fixture_e2e`.

## Project Layout

- `crates/` — Cargo workspace members: `lkit-cli` (the `lkit` binary), `lkit-publish`, `lkit-repository`, `lkit-test-fixture`.
- `docs/` — specifications and design documents at the repository root (moved from `crates/lkit-cli/docs`).
- `scripts/` — integration test scripts.

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
