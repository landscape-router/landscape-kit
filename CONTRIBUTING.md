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

## Questions

For questions that do not belong in an issue, use GitHub Discussions or the issue
tracker of the affected repository.
