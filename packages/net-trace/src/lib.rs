//! Record the client-side network activity of a wrapped command.
//!
//! `net-trace run` starts a localhost forward proxy, points the child's
//! `http_proxy`/`https_proxy`/`all_proxy` at it, and writes one JSON phase
//! file describing every TCP connection the child (and its descendants)
//! opened through the proxy. `net-trace render` folds the accumulated phase
//! files into Markdown, or into a constrained summary JSON for the trusted
//! CI comment job (see `.github/workflows/check.yml`).
//!
//! The proxy never intercepts TLS: a CONNECT tunnel yields the authority
//! (host:port), duration, and byte counts, which is all the visibility CI
//! needs to name a stray eval-time fetch without touching payloads. It only
//! observes processes that honor the proxy environment (curl and git both
//! do, hence Nix eval fetches); the Nix daemon's substitutions and
//! fixed-output builders never route through it.

pub mod proxy;
pub mod report;
