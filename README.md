# Rust Template

A minimal Rust crate with a library, a binary and a unit test.

## What it contains

- `src/lib.rs`: the `greet` function and its test
- `src/main.rs`: the binary; prints a greeting
- `Cargo.toml`, `Cargo.lock`: package metadata (edition 2024, no dependencies) and the lockfile
- `Makefile`: `make verify` runs every check; `make help` lists the targets
- `.github/workflows/pr-ci.yml`: the `PR CI` workflow; runs on every pull request and ends with the `CI gate` job
- `.github/workflows/auto-merge.yml`: merges ready pull requests once `CI gate` passes
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

1. Delete merged branches automatically, allow auto-merge and protect `main`:

   ```bash
   gh repo edit --delete-branch-on-merge --enable-auto-merge
   gh api -X POST 'repos/{owner}/{repo}/rulesets' --input .github/rulesets/main.json
   ```

   The ruleset makes `main` accept changes only through pull requests that pass `CI gate`. GitHub's Free plan does not support rulesets or auto-merge on private repositories: there the second command fails with HTTP 403 and the first has no effect. The auto-merge workflow still waits for `CI gate` before merging, but nothing stops a direct push to `main`.

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

## CI and auto-merge

Work on a branch and open a pull request on GitHub:

```bash
git switch -c <branch>
git push -u origin HEAD
gh pr create --fill
```

- The `PR CI` workflow runs on every pull request. Its `Verify` job installs Rust 1.99.1 with rustfmt and clippy and runs `make verify`. Its last job, `CI gate`, passes only when every other job passed; this is the check the ruleset requires.
- The `Auto-merge` workflow merges a ready pull request from a branch of this repository (squash) once `CI gate` passes and there are no conflicts. Drafts and pull requests from forks are left alone.
- To keep a pull request open, create it as a draft with `gh pr create --draft`, or turn a ready one back into a draft with `gh pr ready --undo <number>`. Run `gh pr ready <number>` when it should merge.

## Deployment

Deployment is configured per project. This template contains no deployment workflow.

## License

GPL-3.0-only. See `LICENSE`.
