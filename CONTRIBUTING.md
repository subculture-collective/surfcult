# Contributing

## Workflow

1. Create a branch from `main`.
2. Make one focused change.
3. Run the checks:

   ```bash
   make verify
   ```

4. Push the branch and open a pull request on GitHub:

   ```bash
   git push -u origin HEAD
   gh pr create --fill
   ```

5. The `PR CI` workflow runs `make verify`. Its last job, `CI gate`, passes only when every other job passed, and `main` accepts a merge only when `CI gate` has passed.
6. The pull request merges itself (squash) once `CI gate` passes and it has no conflicts. To keep working before it merges, open it as a draft with `gh pr create --draft` and run `gh pr ready` when it is done. To stop a ready pull request from merging, run `gh pr merge --disable-auto <number>`.

Pull requests from forks are not merged automatically.

## Commit messages

Use short conventional subjects:

```text
feat: add health endpoint
fix: handle empty names
docs: describe local setup
chore: update dependencies
```

## Issues

Use the bug, feature or task form. Each asks for a checkable "Done when" list and a test command that runs from the repository root.

## Secrets

Never commit secrets or `.env` files. See [SECURITY.md](SECURITY.md).
