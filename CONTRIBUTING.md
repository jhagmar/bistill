# Contributing

Edition 2024. MSRV is 1.85. Registry dependencies are none.

Once the workspace is present:

```
cargo test --workspace --locked
```

`rustfmt` and `clippy -D warnings` apply to new code. Line coverage on
measured packages stays at 100%.
