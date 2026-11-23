# Agent Instructions

This repository is `surfcult`, a Rust project created from `project-template-rust`. The product idea is in `docs/idea.md`; it is a specification, not yet implemented. Update this file when the stack or workflow changes.

## Checks

Run from the repository root before opening a pull request:

```bash
make verify
```

`make verify` runs `cargo fmt --check`, `cargo clippy` with warnings as errors, `cargo test --locked` and `repo-check`. CI runs the same command. Use a narrower command only when a task asks for one.

## Workflow

- GitHub is the source. Create a branch from `main`, push it and open a pull request with `gh pr create --fill`.
- `main` requires the `CI gate` check from `.github/workflows/pr-ci.yml`. If you add a CI job, add it to `needs` and `expected` in the `ci-gate` job.
- When `CI gate` passes and there are no conflicts, the organization merges the pull request automatically. To hold one back, keep it a draft (`gh pr create --draft`, or `gh pr ready --undo <number>` for an open one) or add the `hold` label (`gh pr edit <number> --add-label hold`). Open a draft when the work is not finished.
- Keep one focused change per pull request.

## Boundaries

- Keep the template minimal and runnable.
- Never commit secrets, `.env` files, local databases or build output.
- Deployment is configured per project; this template has no deployment workflow.
