# lean4.hx

Lean-aware integration for the Steel-enabled Helix host. The project is
experimental; it is not yet a packaged editor extension.

## Dependencies

- [utensil/helix](https://github.com/utensil/helix), branch `lean-dev`, pinned
  to `df595c7dc5729e2712c79dd2e35977e3474b3ec6`
- [utensil/steel](https://github.com/utensil/steel), branch `lean-dev`, pinned
  to `24cd21598c091fb88bc10a6375a1ded25e677c37`
- Lean `v4.34.0`
- Rust `1.100.0-nightly`

The exact host inputs are recorded in `HOST-PINS.toml`.

## Development

```sh
cargo check --locked
cargo test --locked
./scripts/host-harness.sh
```

The host extension is in `host/extension.scm`. Build the native library with
`cargo build --release`, install the resulting library under
`$STEEL_HOME/native`, load the Scheme file from a Helix `init.scm`, and call
`(lean4-hx-install!)`. The extension exposes one Lean goal request, editor
selection and character hooks, and a small native component for host testing.

The project is distributed under the [MIT License](LICENSE).
