//! Transport-free authentication application services for Signex.
//!
//! The HTTP binary (`bins/api`) depends on this crate for all social sign-in,
//! session, bootstrap, and MFA behavior. Domain invariants live in
//! `funding_arb_domain::auth`; adapters (provider verification, tokens, and
//! Postgres persistence) live here.

pub mod config;
pub mod dto;
pub mod error;
pub mod mfa;
pub mod providers;
pub mod repo;
pub mod service;
pub mod tokens;

pub use config::{AuthConfig, ConfigError};
pub use error::{AuthError, AuthResult};
pub use providers::{IdentityTokenVerifier, LiveIdentityVerifier, VerifiedIdentity};
pub use repo::Repo;
pub use service::{AuthService, CallerContext};
