//! Application-facing access to Lattice. JSON reads are node claims, not
//! authenticated consensus state. Content and submission use separate APIs.
#![forbid(unsafe_code)]
pub use lattice_primitives as primitives;
pub mod client;
pub mod models;
pub mod wire;
pub use client::*;
pub use models::*;

pub mod volumes;

pub mod endpoints;

pub mod transactions;

pub mod endpoint_walk;

pub mod submission;

pub mod sponsor;

pub mod sponsor_http;

pub mod canonical;

pub mod lattice_client;

pub mod chain_nodes;

/// Authenticated state witnesses with explicit caller-selected commitments.
pub mod proofs;

/// Platform clocks, timers, and background futures.
pub mod runtime;
pub mod transport;
