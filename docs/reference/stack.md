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
