# Contributing

Thanks for contributing to Landscape Kit. AI-assisted ("vibe code") development is
welcome: use an AI to scan the project, open an issue, write code, and submit a pull
request. The project uses an **issue-driven workflow**.

## Workflow

1. **Scan the project first.** Read `README.md`, `docs/README.md`, and `AGENTS.md`
   before proposing anything. The specs in `docs/` define the intended behavior:
   propose changes against them, and update them when behavior changes.

2. **Check for duplicates.** Before opening an issue, search existing issues (use
   `gh issue list --search ...` or the GitHub search box). If you find an existing
   issue, join the discussion there instead of opening a new one.

3. **Open an issue using the template.** Pick the template that matches your request
   ([Bug Report](.github/ISSUE_TEMPLATE/bug-report.en.yml) or
   [Feature Request](.github/ISSUE_TEMPLATE/feature-request.en.yml); Chinese versions:
   [bug-report.zh.yml](.github/ISSUE_TEMPLATE/bug-report.zh.yml),
   [feature-request.zh.yml](.github/ISSUE_TEMPLATE/feature-request.zh.yml)) and fill in
   every field. A
   complete issue describes the affected command, current behavior, expected behavior,
   and acceptance criteria.

4. **Implement.** Follow the conventions in `AGENTS.md`: run `cargo fmt` before
   committing, write unit tests for your change (for example
   `cargo test -p lkit-cli <module-filter>`), and update `docs/` when behavior changes.

5. **Open a pull request.** Reference the issue in the PR description (for example
   `Closes #123`) and fill in the PR template checklist.

## Test helper binaries

The test suites never talk to a real systemd or the real `landscape-webserver`.
Instead, fake programs from the `lkit-test-fixture` crate (`crates/lkit-test-fixture/`)
stand in for them and are driven by JSON config files:

- `lkit-test-systemctl` — a fake `systemctl` (and `systemd-analyze`) whose unit
  files, service state, and command results come from the JSON file named by the
  `LKIT_TEST_SYSTEMCTL_CONFIG` environment variable. Unit tests point the code
  under test at it through the config; the e2e fixture suite calls it directly to
  arrange pre-existing services.
- `lkit-test-init` — a fake SysV `init` used the same way (`LKIT_TEST_INIT_CONFIG`).
- `lkit-landscape-fixture` — stands in for the downloaded
  `landscape-webserver` asset during the e2e fixture suite; it serves a scripted
  release payload so `lkit self update`/install flows run against a fake upstream.

You normally never build these by hand. The e2e suite references them with
`env!("CARGO_BIN_EXE_<name>")`, so `cargo test` builds them automatically, and the
`lkit` binaries are declared in `lkit-cli/Cargo.toml` behind the `test-support`
feature (plus `landscape-webserver` inside the fixture crate itself). If you add a
new fixture program, register it as a `[[bin]]` entry in `lkit-cli/Cargo.toml`
with `required-features = ["test-support"]`, otherwise `CARGO_BIN_EXE_<name>` will
not resolve. The `lkit-test-systemctl`/`lkit-test-init` entries there are three-line
wrappers that delegate to the real programs, which live as `[[bin]]` targets in the
fixture crate itself (together with `landscape-webserver` and `lkit-fixture-release`).

## Console TUI testing

The console layout is pinned by three layers (scenario docs:
[`UI-17`](docs/testing/scenarios/functional/console.md)/[`UI-18`](docs/testing/scenarios/functional/console.md)):

- **Whole-screen snapshots** (`lkit-cli/src/console/tests/snapshots.rs`, 18 screens
  via ratatui's `TestBackend` + `insta`). After an intentional layout change, update
  with `INSTA_UPDATE=always cargo test -p lkit-cli --features test-support --bin lkit
  console::tests::snapshots` (or `cargo insta review`) and review the diffs — the
  `CARGO_PKG_VERSION` string appears in them on every version bump, and backup
  timestamps are normalized to `<DATE>` so snapshots are timezone-independent.
- **Layout invariants** (`invariants.rs`): the 72x18 too-small boundary, a
  40..=120-column sweep asserting footer notices/hints are never clipped, and the
  canonical 100x28 geometry (header rule, 24-column sidebar, status separator,
  right-aligned language indicator).
- **Screen-level e2e** (`tests/install_fixture_e2e/console_screen.rs`) drives the
  bare `lkit` binary through [termlens](https://github.com/vyncint/termlens)
  (real PTY + VT decoding), so assertions anchor on localized screen text —
  including Chinese — instead of raw escape byte streams.

Never run the e2e suite on your host; use the docker entry instead:

```sh
scripts/test-docker-console.sh          # builds in a container, then runs with
                                        # --network none (loopback only) as root
```

The first run populates the `lkit-console-e2e-target`/`lkit-console-e2e-cargo`
volumes; later runs are incremental. CI runs the same suite per push via the
fixture e2e workflow (`console` domain filter matches both `console::` and
`console_screen::`).

## Demo builds

`cargo build -p lkit-cli --features demo` produces a binary where every console
panel background operation (software install, mirror probe/refresh, backup
list/create/verify, update resolution, preflight, daemon deploy) is replaced by
a scripted mock — safe to click through on any machine without touching the
host (scenario doc:
[`UI-19`](docs/testing/scenarios/functional/console.md)). The snapshot is
pinned to `Installed` so gated menus (Update/Reinit) are reachable.

The demo binary is fenced off from real releases three ways: a permanent red
`DEMO` badge in the console header, a `(demo)` suffix in `--version`, and the
release workflow rejecting any release artifact whose `--version` mentions
`demo`. Note the boundary: only TUI panel background operations are mocked —
CLI subcommands, the post-console network-takeover confirmation, and
territory/pidfile reads are still real.

## Questions

For questions that do not belong in an issue, use GitHub Discussions or the issue
tracker of the affected repository.
