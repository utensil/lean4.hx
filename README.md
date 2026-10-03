# lean4.hx

Lean-aware integration for the Steel-enabled Helix host. The project is
experimental; it is not yet a packaged editor extension.

## Dependencies

- [utensil/helix](https://github.com/utensil/helix), branch `lean-dev`, pinned
  to `8bca02cbac15fe21a331d95dae24f42c87fa28e6`
- [utensil/steel](https://github.com/utensil/steel), branch `lean-dev`, pinned
  to `24cd21598c091fb88bc10a6375a1ded25e677c37`
- Lean `v4.34.0`
- Rust `1.100.0-nightly`

## Development

```sh
cargo check --locked
cargo test --locked
./scripts/host-harness.sh
```

Install the native library and Steel module with:

```sh
STEEL_HOME=/path/to/steel ./scripts/install.sh
```

Load the module from the Steel-enabled Helix `init.scm`:

```scheme
(require "lean4-hx.scm")
(lean4-hx-install!)
```

The goal component follows the source cursor. Press `tab` to focus it, `pageup`
or `pagedown` to scroll, and `escape` to return focus to the source buffer.
Removing the component with `(lean4-hx-remove-component!)` cancels pending goal
requests and clears the displayed snapshot.

The project is distributed under the [Apache License 2.0](LICENSE).
