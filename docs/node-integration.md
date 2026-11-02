# Real node SDK workflow

Run from this repository after building the sibling Rust node binary:

```sh
cargo build --locked --example node_workflow
python3 tools/node-integration.py
python3 tools/swift-node-integration.py
```

The second script requires the unchanged pinned Swift daemon at
`/tmp/lattice-node-mixed-build/debug/lattice-node` and the sibling
`lattice-validation` example `mixed_node_signer` (used only for CPU mining).
It checks the original Swift checkout revision and tracked cleanliness.
Both scripts copy binaries into a disposable directory and record SHA-256
hashes in `tests/*node-integration-evidence.json`. All nodes use loopback,
no public peers, disposable keys/data and process cleanup. The Rust fixture
uses the existing easy genesis; Swift uses its canonical Nexus genesis and
real bounded CPU search. Rust restart explicitly removes bootstrap files.

The Rust SDK signs and submits a transfer, authenticates the mined block and
its state/transaction volumes, resolves transaction index zero from the
committed dictionary, reads the recipient balance, and verifies account and
deposit-absence witnesses. The harness supplies the expected mined block CID;
account verification uses its authenticated post-state CID. Wrong roots,
wrong dictionary/block bindings and a forged account balance are rejected.
The same checks pass again after a durable node restart.

Account witnesses are assembled locally from CID-verified HTTP volumes,
following only missing-content requests from the sealed verifier, with a
16-boundary fixture limit. This exercises account proof authentication; it
is not evidence for a node account-proof HTTP endpoint. Deposit witnesses
come from the actual node `/api/deposit-state` response, downloaded by the
Python harness because the SDK has no typed witness-fetch endpoint yet.
The proof verifier cannot access a network or fallback source. Header-chain
canonicality is established by the controlled mining harness, not by the
light-client proof verifier.

Operator RPC requires cookie authentication. Discovery uses the real SDK
ChainNodes with an explicit single-host authenticated NodeClient adapter;
it asserts that credentials never travel to another origin. This does not
claim generic unauthenticated discovery works on an operator RPC port.

The workflow found and fixed `LatticeClient::transactions`: its dictionary
is an ordinary Header inside the block volume, not a separately served
Volume. The client now returns an authenticated view of that block's entries
rooted at the committed dictionary. This view does not imply a new storage
boundary or independent volume membership. The unit regression deliberately
omits a separately served transaction-dictionary volume.
