//! Tamper-evident, per-tenant hash chain over `audit_events` (M4 Slice 7).
//!
//! Each sensitive control-plane action appends a row whose `event_hash` is
//! `SHA-256(previous_hash ‖ canonical(fields))`, so any retroactive mutation or
//! deletion breaks the chain and is caught by [`verify`](crate::repo::Repo::verify_audit_chain).
//! The encoding is length-prefixed and therefore injective — no field boundary
//! can be forged by moving a `:` between fields.

use crate::tokens::sha256;

/// Encoding version tag, mixed in so a future format change is unambiguous.
const CHAIN_TAG: &str = "perpeto-audit-v1";

/// Injective, length-prefixed canonical form of an event's hashable fields. The
/// `details` field must already be a stable serialization (we use
/// `serde_json::Value::to_string()`, which is sorted-key and compact).
#[must_use]
pub fn canonical(
    event_type: &str,
    subject_type: &str,
    subject_id: &str,
    actor_pseudonym: &str,
    request_id: &str,
    session_id: &str,
    details: &str,
    occurred_at: &str,
) -> String {
    length_prefixed(&[
        CHAIN_TAG,
        event_type,
        subject_type,
        subject_id,
        actor_pseudonym,
        request_id,
        session_id,
        details,
        occurred_at,
    ])
}

/// `SHA-256(previous_hash ‖ canonical)`. The genesis entry has no predecessor.
#[must_use]
pub fn event_hash(previous: Option<&[u8; 32]>, canonical: &str) -> [u8; 32] {
    let mut buffer = Vec::with_capacity(32 + canonical.len());
    if let Some(previous) = previous {
        buffer.extend_from_slice(previous);
    }
    buffer.extend_from_slice(canonical.as_bytes());
    sha256(&buffer)
}

/// Concatenate `len:bytes` for each field. The decimal byte length prefix makes
/// the encoding prefix-free, so distinct field tuples never collide.
fn length_prefixed(fields: &[&str]) -> String {
    let mut out = String::new();
    for field in fields {
        out.push_str(&field.len().to_string());
        out.push(':');
        out.push_str(field);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(details: &str, occurred: &str) -> String {
        canonical(
            "CREDENTIAL_ENROLLED",
            "venue_account",
            "acct-1",
            "pseudo-1",
            "req-1",
            "sess-1",
            details,
            occurred,
        )
    }

    #[test]
    fn each_link_depends_on_its_predecessor() {
        let c1 = sample("{}", "2026-07-27T00:00:00.000000+00:00");
        let c2 = sample("{}", "2026-07-27T00:00:01.000000+00:00");
        let h1 = event_hash(None, &c1);
        let linked = event_hash(Some(&h1), &c2);
        let genesis = event_hash(None, &c2);
        // The same event hashes differently depending on the chain it extends.
        assert_ne!(linked, genesis);
        // A different predecessor yields a different link.
        let other = event_hash(Some(&[9_u8; 32]), &c2);
        assert_ne!(linked, other);
    }

    #[test]
    fn any_field_change_changes_the_hash() {
        let base = event_hash(None, &sample("{}", "2026-07-27T00:00:00.000000+00:00"));
        assert_ne!(
            base,
            event_hash(
                None,
                &sample("{\"a\":\"b\"}", "2026-07-27T00:00:00.000000+00:00")
            )
        );
        assert_ne!(
            base,
            event_hash(None, &sample("{}", "2026-07-27T00:00:00.000001+00:00"))
        );
    }

    #[test]
    fn encoding_is_injective_across_delimiter_containing_fields() {
        // Moving a ':' across a field boundary must not produce the same canonical.
        let a = canonical("A", "b:c", "d", "e", "f", "g", "{}", "t");
        let b = canonical("A", "b", "c:d", "e", "f", "g", "{}", "t");
        assert_ne!(a, b);
    }
}
