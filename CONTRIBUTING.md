# Contributing to Cell

We’re happy you’re interested!  
Cell is intentionally small; contributions that keep the code base **tiny, obvious, and fast** are the ones that get merged.

## Code of conduct

Be kind, assume best intent, no corporate BS.

## Getting started

1. Fork & clone  
   ```bash
   git clone https://github.com/<you>/cell
   cd cell
   ```

2. Install nightly (we use `min_specialization` and `stdsimd` benchmarks)  
   ```bash
   rustup toolchain install nightly --component rustfmt,clippy,miri
   rustup default nightly
   ```

3. One-command build & test  
   ```bash
   cargo xtask ready      # formats, lints, tests, builds every example
   ```
   (If `xtask` fails, `cargo install xtask` first.)

## Project conventions

- **No `unwrap()` in production paths** – use `anyhow::Context`.  
- **No new dependencies** unless they compile in `< 2 s` and add **massive** value.  
- **MSRV** – latest stable minus 2 releases.  
- **Features** are additive; `cargo build --all-features` must work on Linux, macOS, Windows.  
- **Unsafe** is allowed **only** in `cell-sdk/src/shm.rs` and must include a 3-line safety comment.  
- **Commits** – conventional style (`feat:`, `fix:`, `docs:`, `perf:`).  
- **PRs** – rebase on main, one logical change per commit, green CI.

## Where to help

| label | skill | what |
|-------|-------|------|
`good-first-issue` | beginner | typos, better errors, doc examples |
`perf` | systems | shave µs, reduce syscalls, zero-copy paths |
`mesh` | networking | QUIC, NAT traversal, DHT gossip |
`gpu` | cuda/opencl | expose PCI device in manifest, scheduler filter |
`crypto` | security | reproducible builds, sig verification, supply-chain |

## Testing

```bash
cargo test --all-features
cargo miri test --cell-sdk   # UB check
cargo criterion              # benchmarks (needs nightly)
```

Add a unit test for every new public item; add an `examples/*/benches` file for every perf change.

## Debugging cells

```bash
RUST_LOG=cell=trace,cell_sdk=trace cell start calculator ./target/release/calculator
# logs appear in /tmp/cell/logs/calculator.log
```

## Docs & book

We use `mdbook`.  
```bash
cargo install mdbook
mdbook serve docs
```
Edit `docs/src/*.md`; netlify auto-deploys on merge.

## Release process (maintainers only)

1. Bump version in **all** `Cargo.toml` files  
   ```bash
   cargo xtask bump 0.2.0
   ```
2. Update CHANGELOG.md  
3. `git tag -s v0.2.0 -m "v0.2.0"`  
4. `git push origin v0.2.0` – GitHub Actions builds & crates.io publish happens automatically.

## Questions?

Open an issue

Happy hacking!