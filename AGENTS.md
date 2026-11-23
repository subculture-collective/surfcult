# Agent Instructions

This repository is the Rust template. Projects created from it start with these files; update this file when the stack or workflow changes.

## Checks

Run from the repository root before opening a pull request:

```bash
make verify
```

`make verify` runs `cargo fmt --check`, `cargo clippy` with warnings as errors, `cargo test --locked` and `repo-check`. CI runs the same command. Use a narrower command only when a task asks for one.

## Workflow

- GitHub is the source. Create a branch from `main`, push it and open a pull request with `gh pr create --fill`.
- `main` requires the `CI gate` check from `.github/workflows/pr-ci.yml`. If you add a CI job, add it to `needs` and `expected` in the `ci-gate` job.
- `.github/workflows/auto-merge.yml` merges a ready pull request (squash) once `CI gate` passes and there are no conflicts. Open a draft (`gh pr create --draft`) when the work is not finished.
- Keep one focused change per pull request.

## Boundaries

- Keep the template minimal and runnable.
- Never commit secrets, `.env` files, local databases or build output.
- Deployment is configured per project; this template has no deployment workflow.
