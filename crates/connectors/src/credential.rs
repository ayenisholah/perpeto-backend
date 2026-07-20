//! Connector-local credential container with mandatory redaction and zeroization.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ConnectorCredentials {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
}

impl fmt::Debug for ConnectorCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConnectorCredentials([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_discloses_credentials() {
        let credentials = ConnectorCredentials {
            api_key: "key-visible".into(),
            api_secret: "secret-visible".into(),
            passphrase: None,
        };
        assert_eq!(
            format!("{credentials:?}"),
            "ConnectorCredentials([REDACTED])"
        );
    }
}
