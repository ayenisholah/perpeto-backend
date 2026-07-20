//! Venue-account persistence and read models.

mod coordinator;
mod dto;
mod reconcile;
mod registry;
mod repo;
mod secret;

pub use coordinator::{
    ConnectorExecutionCoordinator, CoordinatorError, SubmissionResult, SubmissionState,
};
pub use dto::{
    CertificationState, ConnectionTestDto, ConnectorAccountingDto, ConnectorOrderDto,
    ExactBalanceDto, ExactVenuePositionDto, InstrumentDto, ProductConnectionDto,
    ReconciliationRunDto, ReconciliationState, VenueAccountDto, VenueSummaryDto,
};
pub use reconcile::{ReconciliationError, ReconciliationReport, ReconciliationService};
pub use registry::{ConfiguredConnector, ConnectorRegistry, RegistryError, decode_master_key};
pub use repo::ConnectorMonitoringRepo;
pub use repo::VenueAccountsRepo;
pub use secret::{
    CredentialBundle, EncryptedPostgresSecretStore, SecretContext, SecretMetadata, SecretStore,
    SecretStoreError,
};
