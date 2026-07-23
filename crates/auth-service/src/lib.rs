//! Transport-free authentication application services for Perpeto.
//!
//! The HTTP binary (`bins/api`) depends on this crate for all social sign-in,
//! session, bootstrap, and MFA behavior. Domain invariants live in
//! `funding_arb_domain::auth`; adapters (provider verification, tokens, and
//! Postgres persistence) live here.

pub mod config;
pub mod dto;
pub mod error;
pub mod mfa;
pub mod passkey;
pub mod providers;
pub mod repo;
pub mod service;
pub mod tokens;

pub use config::{AuthConfig, ConfigError, PasskeyConfig};
pub use error::{AuthError, AuthResult};
pub use passkey::{
    ActionProofReceipt, AssertionOutcome, CeremonyChallenge, PasskeyService, PasskeyView,
    build_webauthn,
};
pub use providers::{IdentityTokenVerifier, LiveIdentityVerifier, VerifiedIdentity};
pub use repo::Repo;
pub use service::{AuthService, CallerContext};
