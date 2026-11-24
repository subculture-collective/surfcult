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
- When `CI gate` passes and there are no conflicts, the organization merges the pull request automatically. To hold one back, keep it a draft (`gh pr create --draft`, or `gh pr ready --undo <number>` for an open one) or add the `hold` label (`gh pr edit <number> --add-label hold`). Do not open a draft for unfinished work; see below.
- Keep one focused change per pull request.

## Keep CI runs few

CI runs on every push to a branch that has an open pull request, drafts included, and on every push to `main`. A push to a branch with no pull request runs nothing. Each run builds twice (`verify` and the aarch64 release build), so:

- Run `make verify` locally and fix everything before the first push. CI runs the same command, so a local pass should mean a CI pass.
- Push the branch and open the pull request only when the change is finished. Do not open a draft to park unfinished work; push the branch without a pull request instead.
- Once a pull request is open, gather every fix into one push rather than pushing each commit as you make it.
- Do not merge or rebase `main` into an open pull request unless it conflicts or a reviewer asks; the automatic merge does not need an up-to-date branch.
- Changing a pull request's title, body or labels does not start a run; use that for follow-up notes instead of an empty commit.
- Put a documentation fix in the same pull request as the code it describes, not in a separate one.
- Do not re-run a failed job without changing something, unless the failure is clearly a runner or network fault.

## Test device

The target Raspberry Pi 4B is reachable as `ssh surfcult` once an alias is set up. The user is `patrick` and the key is `id_ed25519`. See `docs/runbooks/raspberry-pi.md` for access, installed tools and the MPV flags that work (`--profile=fast`, `--hwdec=drm`). Do not build Rust on the Pi; CI builds the aarch64 binary.

## Boundaries

- Keep the template minimal and runnable.
- Never commit secrets, `.env` files, local databases or build output.
- Deployment is configured per project; this template has no deployment workflow.
