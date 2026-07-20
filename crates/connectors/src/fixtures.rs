//! Redacted, hashed connector contract fixtures used by network-free CI.

use funding_arb_domain::market::Venue;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Fixture provenance and integrity declaration.
#[derive(Clone, Debug, Deserialize)]
pub struct FixtureManifest {
    pub venue: String,
    pub schema_version: u32,
    pub api_revision: String,
    pub official_document: String,
    pub reviewed_at: String,
    pub cases_sha256: String,
    pub required_scenarios: Vec<String>,
}

/// One deterministic REST/WebSocket interaction.
#[derive(Clone, Debug, Deserialize)]
pub struct ContractCase {
    pub name: String,
    pub scenario: String,
    pub request: FixtureRequest,
    pub response: FixtureResponse,
    #[serde(default)]
    pub websocket_frames: Vec<serde_json::Value>,
    pub expectation: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FixtureRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub query: serde_json::Value,
    #[serde(default)]
    pub body: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FixtureResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

/// Loaded fixture suite with verified content hash and coverage labels.
#[derive(Clone, Debug)]
pub struct ContractSuite {
    pub manifest: FixtureManifest,
    pub cases: Vec<ContractCase>,
}

impl ContractSuite {
    /// Load an embedded venue suite and verify integrity before use.
    pub fn load(venue: Venue) -> Result<Self, String> {
        let (manifest_raw, cases_raw) = match venue {
            Venue::Binance => (
                include_str!("../fixtures/binance/manifest.json"),
                include_str!("../fixtures/binance/cases.json"),
            ),
            Venue::Bybit => (
                include_str!("../fixtures/bybit/manifest.json"),
                include_str!("../fixtures/bybit/cases.json"),
            ),
            Venue::Okx => (
                include_str!("../fixtures/okx/manifest.json"),
                include_str!("../fixtures/okx/cases.json"),
            ),
            Venue::DyDx | Venue::Hyperliquid => return Err("M4 venue has no M3 suite".to_owned()),
        };
        let manifest: FixtureManifest =
            serde_json::from_str(manifest_raw).map_err(|error| error.to_string())?;
        let cases: Vec<ContractCase> =
            serde_json::from_str(cases_raw).map_err(|error| error.to_string())?;
        let digest = Sha256::digest(cases_raw.as_bytes());
        let actual = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual != manifest.cases_sha256 {
            return Err(format!("{} fixture hash mismatch", manifest.venue));
        }
        if manifest.venue != venue.as_str() {
            return Err("fixture venue mismatch".to_owned());
        }
        for required in &manifest.required_scenarios {
            if !cases.iter().any(|case| &case.scenario == required) {
                return Err(format!("missing required scenario {required}"));
            }
        }
        let serialized = format!("{manifest_raw}{cases_raw}").to_ascii_lowercase();
        for forbidden in ["api_secret", "secretkey", "private_key", "x-bapi-api-key"] {
            if serialized.contains(forbidden) {
                return Err(format!(
                    "fixture contains forbidden secret marker {forbidden}"
                ));
            }
        }
        Ok(Self { manifest, cases })
    }
}

#[cfg(test)]
mod tests {
    use super::ContractSuite;
    use funding_arb_domain::market::Venue;

    #[test]
    fn every_m3_venue_fixture_is_hashed_redacted_and_complete() {
        for venue in [Venue::Bybit, Venue::Binance, Venue::Okx] {
            let suite = ContractSuite::load(venue).expect("valid embedded fixture suite");
            assert_eq!(suite.manifest.schema_version, 1);
            assert!(!suite.cases.is_empty());
            assert!(suite.manifest.official_document.starts_with("https://"));
        }
    }

    #[test]
    fn m4_venues_are_not_silently_covered_by_cex_fixtures() {
        assert!(ContractSuite::load(Venue::DyDx).is_err());
        assert!(ContractSuite::load(Venue::Hyperliquid).is_err());
    }
}
