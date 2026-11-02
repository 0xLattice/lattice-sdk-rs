# Browser verification

This fixture builds the real Rust SDK into a wasm-bindgen module and runs it in a
browser against a local synthetic HTTP server. It does not contact a live chain.

```
rustup target add wasm32-unknown-unknown
# Install wasm-bindgen-cli matching Cargo.lock's wasm-bindgen version.
bash browser/build.sh
python3 browser/server.py
```

Open the printed localhost URL for the window run, or append `/worker.html`
for the dedicated module worker run. Each page must display PASS. The worker
checks its execution context before loading the Rust WASM module; the parent
page only displays its result and checks server counters. Restart the server
before each run because request counters intentionally describe one test run.
The server checks that redirects were never followed and observes four aborted
connections after deadlines/cancellation before and after response headers.
Tests also exercise explicit authorization, streamed response caps, authenticated
volume retrieval, explicit transaction submission, shared discovery, browser
cryptographic entropy, sponsor search and unavailable DNS preflight.

`evidence.json` records the observed Chrome window run; `worker-evidence.json`
records the dedicated worker run, its input hashes and the WASM artifact hash.
Both runs cover the same eleven SDK checks and four server-observed aborts.
This is not evidence for every browser, shared/service workers, cross-origin
CORS configurations or every SDK workflow. Generated
`pkg/` files are build products and are ignored.
