# lean4.hx

`lean4.hx` is an experimental public repository for a future Lean proof-state
and Helix editor bridge. The repository currently contains policy and
provenance documents plus a documentation-only Cargo placeholder. There is no
feature implementation, host harness, CI configuration, or implementation
test suite yet.

Development discussion is anchored to public upstream inputs so that later
work can be reproduced:

- Helix `utensil/helix:lean-dev`, commit
  `df595c7dc5729e2712c79dd2e35977e3474b3ec6`, based on the public
  `mattwparas/helix` `steel-event-system` branch.
- Steel `utensil/steel:lean-dev`, commit
  `24cd21598c091fb88bc10a6375a1ded25e677c37`, based on public
  `mattwparas/steel`.
- Lean 4.34.0 as the pinned official toolchain input.

Those pins are documentation-only at this stage. They are not fetched or
executed by this repository. See [PROVENANCE.md](PROVENANCE.md) for source
URLs and licenses, and [CONTRIBUTING.md](CONTRIBUTING.md) before proposing a
change.

This project is distributed under the [MIT License](LICENSE).
