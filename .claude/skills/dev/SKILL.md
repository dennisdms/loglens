---
name: dev
description: Development conventions for this repository. Use when writing or reviewing code.
---

# Dev

## Comments

Write code that documents itself. Do not write comments that explain the code.

Only use comments when the behavior isn't obvious, and be as concise as possible.

## Testing

Unit and integration tests live in `#[cfg(test)]` modules next to the code they
cover. Run all three before committing:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Performance benchmarking

Benchmark whenever changing performance-sensitive code:
`references/performance-benchmarking.md`.
