//! Shared HTTP client settings for the venue connectors.
//!
//! Every connector request is bounded, so a stalled venue surfaces as an error
//! instead of a hung task. For an order submission, a timeout after the request
//! may have left this host is an *unknown outcome*: the order may or may not
//! exist, and the coordinator must query before any retry (M4).

use std::time::Duration;

use crate::VenueEnvironment;

/// Default whole-request bound when `PERPETO_CONNECTOR_TIMEOUT_MS` is unset.
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Connection-establishment bound.
const CONNECT_TIMEOUT_MS: u64 = 5_000;

/// The HTTP client every connector uses.
#[must_use]
pub fn client() -> reqwest::Client {
    client_with(request_timeout(
        std::env::var("PERPETO_CONNECTOR_TIMEOUT_MS")
            .ok()
            .as_deref(),
    ))
}

fn client_with(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_millis(CONNECT_TIMEOUT_MS).min(timeout))
        .build()
        .unwrap_or_default()
}

/// Parses `PERPETO_CONNECTOR_TIMEOUT_MS`, falling back to the default for an
/// unset, zero or malformed value.
#[must_use]
pub fn request_timeout(value: Option<&str>) -> Duration {
    Duration::from_millis(
        value
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|millis| *millis > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS),
    )
}

/// Fault injection for M4 recovery evidence: a short timeout applied to order
/// submissions only, so the order can reach the venue while its response is
/// lost. Honoured only for TESTNET and DEMO environments; production ignores
/// it unconditionally.
#[must_use]
pub fn submit_fault_timeout(environment: VenueEnvironment) -> Option<Duration> {
    fault_timeout(
        environment,
        std::env::var("PERPETO_FAULT_SUBMIT_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

fn fault_timeout(environment: VenueEnvironment, value: Option<&str>) -> Option<Duration> {
    if !environment.is_non_production() {
        return None;
    }
    value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_timeout_defaults_and_parses() {
        assert_eq!(request_timeout(None), Duration::from_secs(10));
        assert_eq!(
            request_timeout(Some(" 2500 ")),
            Duration::from_millis(2_500)
        );
        assert_eq!(request_timeout(Some("0")), Duration::from_secs(10));
        assert_eq!(request_timeout(Some("soon")), Duration::from_secs(10));
    }

    #[test]
    fn the_submit_fault_never_applies_to_production() {
        assert_eq!(
            fault_timeout(VenueEnvironment::Production, Some("120")),
            None
        );
        assert_eq!(
            fault_timeout(VenueEnvironment::Testnet, Some("120")),
            Some(Duration::from_millis(120))
        );
        assert_eq!(
            fault_timeout(VenueEnvironment::Demo, Some("120")),
            Some(Duration::from_millis(120))
        );
        assert_eq!(fault_timeout(VenueEnvironment::Testnet, None), None);
        assert_eq!(fault_timeout(VenueEnvironment::Testnet, Some("0")), None);
    }

    #[test]
    fn the_client_builds_with_any_timeout() {
        let _ = client_with(Duration::from_millis(1));
        let _ = client();
    }
}
