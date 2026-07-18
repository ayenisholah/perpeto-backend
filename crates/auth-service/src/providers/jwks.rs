//! Cached JSON Web Key Set fetching with automatic refresh on unknown key ids.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::DecodingKey;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::AuthError;

const CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
    #[serde(default)]
    alg: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

struct CacheEntry {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

/// A TTL cache of decoding keys for a single JWKS endpoint.
pub struct JwksCache {
    url: String,
    http: reqwest::Client,
    entry: RwLock<Option<CacheEntry>>,
}

impl JwksCache {
    /// Create a cache for the given JWKS URL.
    #[must_use]
    pub fn new(url: impl Into<String>, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            url: url.into(),
            http,
            entry: RwLock::new(None),
        })
    }

    /// Resolve a decoding key by key id, refreshing the cache if needed.
    ///
    /// # Errors
    /// Returns [`AuthError::ProviderProofInvalid`] if the key id is unknown even
    /// after a refresh, or [`AuthError::Internal`] on transport failures.
    pub async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(entry) = self.entry.read().await.as_ref()
            && entry.fetched_at.elapsed() < CACHE_TTL
            && let Some(key) = entry.keys.get(kid)
        {
            return Ok(key.clone());
        }
        self.refresh().await?;
        self.entry
            .read()
            .await
            .as_ref()
            .and_then(|entry| entry.keys.get(kid).cloned())
            .ok_or(AuthError::ProviderProofInvalid)
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        let response = self
            .http
            .get(&self.url)
            .send()
            .await
            .map_err(|error| AuthError::Internal(format!("jwks fetch failed: {error}")))?;
        let set: JwkSet = response
            .json()
            .await
            .map_err(|error| AuthError::Internal(format!("jwks decode failed: {error}")))?;
        let mut keys = HashMap::new();
        for jwk in set.keys {
            if let Some(alg) = &jwk.alg
                && alg != "RS256"
            {
                continue;
            }
            if let Ok(key) = DecodingKey::from_rsa_components(&jwk.n, &jwk.e) {
                keys.insert(jwk.kid, key);
            }
        }
        *self.entry.write().await = Some(CacheEntry {
            keys,
            fetched_at: Instant::now(),
        });
        Ok(())
    }
}
