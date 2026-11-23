# Rust Template

A minimal Rust crate with a library, a binary and a unit test.

## What it contains

- `src/lib.rs`: the `greet` function and its test
- `src/main.rs`: the binary; prints a greeting
- `Cargo.toml`, `Cargo.lock`: package metadata (edition 2024, no dependencies) and the lockfile
- `Makefile`: `make verify` runs every check; `make help` lists the targets
- `.github/workflows/pr-ci.yml`: the `PR CI` workflow; runs on every pull request and ends with the `CI gate` job
- `.github/rulesets/main.json`: a ruleset for `main` that requires pull requests and `CI gate`
- `.github/ISSUE_TEMPLATE/`, `.github/pull_request_template.md`: bug, feature and task forms and the pull request template
- `docs/`: folders for decision records, references, research notes and runbooks
- `CONTRIBUTING.md`, `SECURITY.md`, `AGENTS.md`: contribution, security and coding-agent guidance

## Create a project

```bash
gh repo create subculture-collective/<name> --template subculture-collective/project-template-rust --private --clone
cd <name>
```

## First steps

1. Delete merged branches automatically, allow auto-merge, protect `main` and add the `hold` label:

   ```bash
   gh repo edit --delete-branch-on-merge --enable-auto-merge
   gh api -X POST 'repos/{owner}/{repo}/rulesets' --input .github/rulesets/main.json
   gh label create hold --description "Do not merge automatically"
   ```

   The ruleset makes `main` accept changes only through pull requests that pass `CI gate`. GitHub's Free plan does not support rulesets or auto-merge on private repositories: there the ruleset command fails with HTTP 403 and nothing stops a direct push to `main`. Pull requests are still merged only after `CI gate` passes (see below).

2. On a branch, rename the template:
   - `name` in `Cargo.toml` and `project_template_rust` in `src/main.rs`
   - `PROJECT_NAME` in `.env.example`
   - this README
3. Run `make verify`, then open a pull request (see below).

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
