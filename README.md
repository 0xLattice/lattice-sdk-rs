# lattice-sdk (Rust)

Rust application SDK for the 0xLattice platform. This port is in progress; see
[PORTING.md](PORTING.md) for source pins, tested coverage and missing components.

The native `NodeClient` performs bounded, cancellable reads. Protocol primitives
come from the sibling `lattice-rs` repository. Reads, verified content and
transaction submission are separate capabilities.

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

This checkout currently requires sibling `lattice-rs`, `cashew-rs`, `multikey-rs`
and `array-trie-rs` repositories through their local path dependencies.

Browser/WASM support uses Fetch with bounded streaming responses, abort-on-drop,
browser timers and local futures; native builds retain Tokio and Rustls. See
[platform limits](PORTING.md#browser-runtime-and-transport) and the
[actual browser test](browser/README.md). DNS preflight is native-only. Browser
CORS and credential rules apply; this crate does not automatically export a
JavaScript-compatible SDK surface.
