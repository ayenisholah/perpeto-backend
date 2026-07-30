mod handlers;
mod middleware;
mod problem;
mod routes;
mod state;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;
use std::{env, thread, time::Duration};

use funding_arb_auth_service::{
    AuthConfig, AuthService, DevIdentityVerifier, IdentityTokenVerifier, LiveIdentityVerifier,
    PasskeyService, Repo, build_webauthn,
};
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{
    BreakersRepo, ExecutionService, MonitoringRepo, OutboxRepo, PositionsRepo,
};
use funding_arb_scanner_service::ScannerRepo;
use funding_arb_strategy_service::StrategiesRepo;
use funding_arb_tenant_context::assert_safe_runtime_role;
use funding_arb_venue_service::{
    ConnectorExecutionCoordinator, ConnectorMonitoringRepo, ConnectorRegistry,
    CredentialEnrollmentService, ReconciliationService, VenueAccountsRepo,
};
use sqlx::postgres::PgPoolOptions;

use crate::routes::build_router;
use crate::state::AppState;

fn main() -> ExitCode {
    if env::args().any(|argument| argument == "--health-check") {
        return health_check();
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run())
}

async fn run() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| String::from("PAPER"));
    let mode = match TradingMode::from_str(&mode) {
        Ok(mode) if !mode.is_live() => mode,
        Ok(_) => {
            eprintln!("LIVE mode remains locked until certification");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            eprintln!("invalid PERPETO_MODE");
            return ExitCode::FAILURE;
        }
    };

    let config = match AuthConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let pool = match PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("failed to connect to the database: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = assert_safe_runtime_role(&pool).await {
        eprintln!("unsafe API database role: {error}");
        return ExitCode::FAILURE;
    }
    let scanner = ScannerRepo::new(pool.clone());
    let positions = PositionsRepo::new(pool.clone());
    let breakers = BreakersRepo::new(pool.clone());
    let outbox = OutboxRepo::new(pool.clone());
    let strategies = StrategiesRepo::new(pool.clone());
    let monitoring = MonitoringRepo::new(pool.clone());
    let venues = VenueAccountsRepo::new(pool.clone());
    let connector_monitoring = ConnectorMonitoringRepo::new(pool.clone());
    let connector_execution = ConnectorExecutionCoordinator::new(pool.clone());
    let reconciliation = ReconciliationService::new(pool.clone());
    let connector_registry = if mode.is_paper() {
        None
    } else {
        // Prefer Vault Transit when configured; otherwise the transitional AES
        // master key. Outside PAPER exactly one must resolve, else fail closed.
        match ConnectorRegistry::from_env(pool.clone()) {
            Ok(registry) => Some(Arc::new(registry)),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    };
    // Passkey-gated credential enrollment shares the non-PAPER secret backend.
    let enrollment = if mode.is_paper() {
        None
    } else {
        match CredentialEnrollmentService::from_env(pool.clone()) {
            Ok(service) => Some(Arc::new(service)),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    };
    let execution = ExecutionService::default();
    let passkeys = match config.passkey.as_ref() {
        Some(passkey_config) => match build_webauthn(passkey_config) {
            Ok(webauthn) => Some(PasskeyService::new(pool.clone(), webauthn)),
            Err(error) => {
                eprintln!("invalid passkey configuration: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let repo = Repo::new(pool);

    if let (Some(hash), Some(expiry)) = (
        config.bootstrap_token_hash.as_ref(),
        config.bootstrap_token_expires_at.as_ref(),
    ) && let Err(error) = repo.seed_bootstrap(hash, expiry).await
    {
        eprintln!("failed to seed bootstrap state: {error}");
        return ExitCode::FAILURE;
    }

    // Development sign-in for Expo Go, which cannot run the native Google or
    // Apple modules. This substitutes provider signature verification, so it is
    // an authentication bypass and is gated twice: PAPER mode *and* an explicit
    // opt-in. Setting the flag in any other mode aborts startup rather than
    // silently falling back to the live verifier, so it cannot be enabled by
    // accident on a deployment. See DEC-0014.
    let dev_auth = env::var("PERPETO_DEV_AUTH")
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if dev_auth && !mode.is_paper() {
        eprintln!(
            "PERPETO_DEV_AUTH is only permitted in PAPER mode (current mode is {mode}); \
             refusing to start with provider verification disabled"
        );
        return ExitCode::FAILURE;
    }
    let verifier: Arc<dyn IdentityTokenVerifier> = if dev_auth {
        eprintln!(
            "WARNING: PERPETO_DEV_AUTH is enabled. Provider identity tokens are NOT verified \
             and any caller can sign in as any subject. Local development only."
        );
        Arc::new(DevIdentityVerifier)
    } else {
        Arc::new(LiveIdentityVerifier::new(
            config.apple_bundle_id.clone(),
            config.google_client_ids.clone(),
        ))
    };
    let service = AuthService::new(repo, &config, verifier);
    let router = build_router(AppState {
        mode,
        service,
        passkeys,
        scanner,
        execution,
        positions,
        breakers,
        outbox,
        strategies,
        monitoring,
        venues,
        connector_monitoring,
        connector_registry,
        enrollment,
        connector_execution,
        reconciliation,
    });

    let listener = match tokio::net::TcpListener::bind(&config.bind_address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind {}: {error}", config.bind_address);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Perpeto API listening on {} in {} mode",
        config.bind_address, mode,
    );
    match axum::serve(listener, router).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("server error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn health_check() -> ExitCode {
    let address = env::var("PERPETO_API_BIND").unwrap_or_else(|_| String::from("0.0.0.0:8080"));
    let port = address.rsplit(':').next().unwrap_or("8080");
    let target = format!("127.0.0.1:{port}");
    match TcpStream::connect(&target) {
        Ok(mut stream) => {
            let request = b"GET /livez HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(request).is_err() {
                return ExitCode::FAILURE;
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buffer = [0_u8; 64];
            thread::sleep(Duration::from_millis(20));
            match stream.read(&mut buffer) {
                Ok(length) if String::from_utf8_lossy(&buffer[..length]).contains("200") => {
                    ExitCode::SUCCESS
                }
                _ => ExitCode::FAILURE,
            }
        }
        Err(_) => ExitCode::FAILURE,
    }
}
