#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --target wasm32-unknown-unknown --example browser_smoke
wasm-bindgen target/wasm32-unknown-unknown/debug/examples/browser_smoke.wasm --target web --out-dir browser/pkg
