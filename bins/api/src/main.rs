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

use funding_arb_auth_service::{AuthConfig, AuthService, LiveIdentityVerifier, Repo};
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{
    BreakersRepo, ExecutionService, MonitoringRepo, OutboxRepo, PositionsRepo,
};
use funding_arb_scanner_service::ScannerRepo;
use funding_arb_strategy_service::StrategiesRepo;
use funding_arb_venue_service::{
    ConnectorExecutionCoordinator, ConnectorRegistry, VenueAccountsRepo, decode_master_key,
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
    let scanner = ScannerRepo::new(pool.clone());
    let positions = PositionsRepo::new(pool.clone());
    let breakers = BreakersRepo::new(pool.clone());
    let outbox = OutboxRepo::new(pool.clone());
    let strategies = StrategiesRepo::new(pool.clone());
    let monitoring = MonitoringRepo::new(pool.clone());
    let venues = VenueAccountsRepo::new(pool.clone());
    let connector_execution = ConnectorExecutionCoordinator::new(pool.clone());
    let connector_registry = if mode.is_paper() {
        None
    } else {
        let encoded = match env::var("PERPETO_SECRET_MASTER_KEY") {
            Ok(value) => value,
            Err(_) => {
                eprintln!("PERPETO_SECRET_MASTER_KEY is required outside PAPER mode");
                return ExitCode::FAILURE;
            }
        };
        let master_key = match decode_master_key(&encoded) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        };
        Some(Arc::new(ConnectorRegistry::new(pool.clone(), master_key)))
    };
    let execution = ExecutionService::default();
    let repo = Repo::new(pool);

    if let (Some(hash), Some(expiry)) = (
        config.bootstrap_token_hash.as_ref(),
        config.bootstrap_token_expires_at.as_ref(),
    ) && let Err(error) = repo.seed_bootstrap(hash, expiry).await
    {
        eprintln!("failed to seed bootstrap state: {error}");
        return ExitCode::FAILURE;
    }

    let verifier = Arc::new(LiveIdentityVerifier::new(
        config.apple_bundle_id.clone(),
        config.google_client_ids.clone(),
    ));
    let service = AuthService::new(repo, &config, verifier);
    let router = build_router(AppState {
        mode,
        service,
        scanner,
        execution,
        positions,
        breakers,
        outbox,
        strategies,
        monitoring,
        venues,
        connector_registry,
        connector_execution,
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
