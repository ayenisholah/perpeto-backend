//! Venue-account persistence and read models.

mod coordinator;
mod dto;
mod enrollment;
mod preflight;
mod public_market;
mod reconcile;
mod registry;
mod repo;
mod secret;
mod vault;

pub use coordinator::{
    ConnectorExecutionCoordinator, CoordinatorError, SubmissionResult, SubmissionState,
    UnresolvedIntent,
};
pub use dto::{
    CertificationError, CertificationState, ConnectionTestDto, ConnectorAccountingDto,
    ConnectorOrderDto, ExactBalanceDto, ExactVenuePositionDto, InstrumentDto, ProductConnectionDto,
    ReconciliationRunDto, ReconciliationState, VenueAccountDto, VenueSummaryDto,
};
pub use enrollment::{CredentialEnrollmentService, EnrollRequest, EnrollmentError};
pub use preflight::{
    PreflightError, PreflightOutcome, PreflightReconciliationSummary, PreflightService,
    evaluate_config,
};
pub use public_market::{
    BookSide, DEFAULT_DEPTH, DEFAULT_UNDERLYINGS, DEFAULT_VENUE_TIMEOUT, FeedStatus, LegBook,
    MarketSnapshot, PublicMarketConfig, PublicMarketSource, VenueFeedStatus, book_measurement,
    measure_leg,
};
pub use reconcile::{ReconciliationError, ReconciliationReport, ReconciliationService};
pub use registry::{
    ConfiguredConnector, ConnectorRegistry, RegistryError, decode_master_key, secret_store_from_env,
};
pub use repo::ConnectorMonitoringRepo;
pub use repo::VenueAccountsRepo;
pub use secret::{
    CredentialBundle, EncryptedPostgresSecretStore, SecretContext, SecretMetadata, SecretStore,
    SecretStoreError,
};
pub use vault::{VaultConfig, VaultConfigError, VaultTransitSecretStore};
