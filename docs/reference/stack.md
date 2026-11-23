# Stack Reference

Template: `project-template-rust`

A minimal Rust crate with a library, a binary and a unit test.

## Versions

| Tool | Version |
| --- | --- |
| Rust | 1.99.1 |
| dtolnay/rust-toolchain | pinned by commit |
| actions/checkout | v7 |

Keep these versions the same across the SUBCULT project templates.

## Checks

```bash
make verify
```

`make verify` runs `cargo fmt --check`, `cargo clippy` with warnings as errors, `cargo test --locked` and `repo-check`. The `Verify` job in `.github/workflows/pr-ci.yml` installs Rust 1.99.1 with rustfmt and clippy and runs `make verify`.

## Raspberry Pi build

The `Build aarch64` job in the same workflow builds the release binary on a native arm64 runner and uploads it as the `surfcult-aarch64` artifact (kept 14 days). The Pi 4B target does not build Rust itself. The job is part of `CI gate`. Nothing deploys the artifact yet.
