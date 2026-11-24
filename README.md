# surfcult

A self-hosted, offline-first television channel simulator for Raspberry Pi and Linux. It plays your own media as continuously broadcasting channels with schedules, commercials and a program guide. The planned design is in [`docs/idea.md`](docs/idea.md), which uses the codename BROADCAST. Nothing in it is implemented yet.

## What it contains

- `src/lib.rs`: declares the library modules
- `src/main.rs`: the binary, `surfcult check-config <channels.toml>`
- `Cargo.toml`, `Cargo.lock`: package metadata (edition 2024, no dependencies) and the lockfile
- `Makefile`: `make verify` runs every check; `make help` lists the targets
- `.github/workflows/pr-ci.yml`: the `PR CI` workflow; runs on every pull request and ends with the `CI gate` job
- `.github/rulesets/main.json`: the ruleset applied to `main`; it requires pull requests and `CI gate`
- `.github/ISSUE_TEMPLATE/`, `.github/pull_request_template.md`: bug, feature and task forms and the pull request template
- `CONTEXT.md`: a glossary of the domain terms the code uses
- `docs/`: the product idea, plus folders for decision records, references, research notes and runbooks
- `CONTRIBUTING.md`, `SECURITY.md`, `AGENTS.md`: contribution, security and coding-agent guidance

## Run checks locally

Requirements:

- Rust: 1.99.1 with rustfmt and clippy

```bash
make verify
```

`make verify` runs `cargo fmt --check`, `cargo clippy` with warnings as errors, `cargo test --locked` and `repo-check`. Run `make help` to list the individual targets.

## CI and merging

Work on a branch and open a pull request on GitHub:

```bash
git switch -c <branch>
git push -u origin HEAD
gh pr create --fill
```

- The `PR CI` workflow runs on every pull request. Its `Verify` job installs Rust 1.99.1 with rustfmt and clippy and runs `make verify`. Its last job, `CI gate`, passes only when every other job passed; this is the check the ruleset requires.
- When `CI gate` passes on the latest commit and there are no conflicts, the organization merges the pull request automatically. Pull requests from forks are not merged automatically.
- To hold a pull request back, keep it a draft (`gh pr create --draft`, or `gh pr ready --undo <number>` for an open one) or add the `hold` label (`gh pr edit <number> --add-label hold`). Run `gh pr ready <number>` or remove the label when it should merge.

## Deployment

Deployment is configured per project. This template contains no deployment workflow.

## License

GPL-3.0-only. See `LICENSE`.
