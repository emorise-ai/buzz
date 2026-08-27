//! Shared provider-protocol logic for Buzz backend bindings.
//!
//! A backend binding (`buzz-backend-<id>`) delegates an agent to a substrate:
//! `buzz-backend-kubernetes` runs it as a pod, `buzz-backend-docker` as a
//! sandboxed container. See `docs/remote-agents.md` for the contract.
//!
//! Most of what a binding does is **not** substrate-specific. The stdin/stdout
//! wire protocol, the three-tier environment precedence, the reserved-key
//! rules, and the respond-to gate are all the spec's, identical everywhere. Two
//! bindings previously carried a near-verbatim copy of each — 250 lines of
//! `wire.rs` differing in four, and 740 lines of `env.rs` differing in two
//! error strings — with their tests duplicated alongside.
//!
//! That is a correctness hazard rather than merely untidy: a spec fix applied
//! to one copy and missed in the other makes two bindings disagree about the
//! same contract, and the tests pass in both.
//!
//! Genuinely substrate-specific behavior stays in the binding: how to reach the
//! substrate, what a container is called, and how limits are expressed. Where a
//! shared rule needs a substrate-specific *explanation*, the binding supplies
//! the wording via [`env::SubstrateDiagnostics`] rather than forking the rule.

#![deny(unsafe_code)]

pub mod env;
pub mod wire;

pub use env::{build_env, AuthoritativeInputs, SubstrateDiagnostics};
pub use wire::{
    AgentPayload, DeployRequest, DeployResponse, ErrorResponse, InfoResponse, LaunchBlock, Request,
    Response, PROTOCOL_VERSION,
};
