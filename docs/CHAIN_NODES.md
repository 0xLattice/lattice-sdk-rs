# ChainNodes

Ported from `0xLattice/lattice-sdk@3a1be07318f12b85f147e0a5ea55ae68a224e780`,
`packages/client/src/chain-nodes.ts`.

`chain_nodes::ChainNodes` accepts normalized start URLs, an injectable
`ChainNodeDiscovery`, and clock/random/TTL options. `HttpChainNodeDiscovery`
provides real bounded NodeClient chain-info requests and EndpointResolver
lookups. Start checks default to 15 seconds, lists to 30 seconds; resolver
options retain their own per-request deadlines and budgets. No operator
credentials are forwarded by this adapter.

- Start checks run concurrently and preserve start order when selecting the
  first failure. Only 404 means absent; a mismatched chain is also non-serving.
- Discovery walks from the nearest ancestor each answering start serves.
  Failed start checks are not directory sources. Walk/read turns are separate
  per chain, seeded by one random 32-bit starting turn and retained past expiry.
- Start nodes precede found hosts. A found URL already in the start result is
  removed; `found` records whether a node came from directory discovery.
- An empty directory answer ends discovery unless an earlier walk failed. Then
  other starts are tried and an all-empty result preserves that first failure.
  Directory failures are tolerated when a start already serves the target.
- Concurrent callers share the same list/error object. Successful TTL begins
  at completion, includes empty lists, and expires on equality. Errors are
  removed immediately. Discovery continues on Tokio if a waiter is dropped.
- `list` can return an empty successful list. Only `read` converts this to
  `ChainNodesError::Offline`. Each read begins at its next turn and tries all
  nodes, returning the callback's final error if none succeeds.

Results use `Arc<Vec<ChainNode>>`; errors use `Arc<ClientError>` to preserve a
shared transport error without lossy string conversion. Read callbacks return
`ChainNodesError`; custom transport/read logic can preserve ClientError variants.
The clock is unsigned milliseconds and TTL a Rust Duration. Injected random
functions must meet the reference's finite [0,1) contract. Turn counters are
u64, not JavaScript floating-point numbers; behavior after numeric precision or
counter exhaustion is not claimed as source parity.

## Verification

```sh
node scripts/chain-nodes-typescript-fixtures.mjs
cargo test --test chain_nodes --test chain_nodes_http
cargo clippy --lib --test chain_nodes --test chain_nodes_http -- -D warnings
```

The generator executes the actual pinned TypeScript ChainNodes class with
`NodeClient.chainInfo` and `EndpointResolver.resolve` replaced by deterministic
injected seams. It records 13 scenarios' discovery calls, returned lists,
read attempts, selected nodes and errors. These are compared exactly in Rust.
This corpus checks ChainNodes composition, not HTTP or directory implementation;
those have independent tests/corpora. Source/fixture hashes and Node version
are in `tests/chain-nodes-typescript-evidence.json`.

Scenarios cover deduplication, all-offline, all-failed, partial failure plus an
empty result, own-node fallback after directory failure, failed/empty/nonempty
walk combinations, nearest ancestor fallback, random turns, all-read-failure
ordering, expiry walk rotation and retry after failed discovery.

Three additional Rust concurrency tests check simultaneous start checks,
single-flight sharing, shared error identity, retry, TTL from completion and
continued discovery after a waiter is cancelled. Two real loopback HTTP tests
check successful/mismatched chain responses, 404, 503, exact request path,
absence of authorization and timeout classification. No live service is used.
