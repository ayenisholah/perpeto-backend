//! Router-level tests: read-only demo authorization (DEC-0017) and paper-entry
//! freshness (DEC-0016).
//!
//! The mutating routes are read from `routes.rs` itself, so a route added
//! later is covered without editing this file. Database-backed: each test
//! skips when `DATABASE_URL` is unset, like the other suites.

use std::env;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use funding_arb_auth_service::dto::{
    CreateChallengeRequest, DeviceRegistration, ExchangeRequest, Provider,
};
use funding_arb_auth_service::tokens::{encode_base64url, sha256};
use funding_arb_auth_service::{AuthConfig, AuthService, DevIdentityVerifier, Repo};
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{
    BreakersRepo, ExecutionService, MonitoringRepo, OutboxRepo, PositionsRepo,
};
use funding_arb_scanner_service::ScannerRepo;
use funding_arb_strategy_service::StrategiesRepo;
use funding_arb_venue_service::{
    ConnectorExecutionCoordinator, ConnectorMonitoringRepo, ReconciliationService,
    VenueAccountsRepo,
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

use crate::demo::{self, DemoGate};
use crate::routes::build_router;
use crate::state::AppState;

const ROUTES_SOURCE: &str = include_str!("routes.rs");

/// Every `(method, path)` registered in `routes.rs`, and whether it sits
/// behind `require_auth` (every route declared before the auth layer).
fn declared_routes() -> Vec<(Method, String, bool)> {
    let layer = ROUTES_SOURCE
        .find("require_auth));")
        .expect("routes.rs applies require_auth");
    let mut routes = Vec::new();
    let mut rest = ROUTES_SOURCE;
    let mut offset = 0;
    while let Some(start) = rest.find(".route(") {
        let block_start = offset + start;
        let block = &ROUTES_SOURCE[block_start..];
        let path_start = block.find('"').expect("route path") + 1;
        let path_end = path_start + block[path_start..].find('"').expect("route path end");
        let path = block[path_start..path_end].to_owned();
        let next = block[1..]
            .find(".route(")
            .map_or(block.len(), |index| index + 1);
        let body = &block[path_end..next];
        for (needle, method) in [
            ("get(handlers::", Method::GET),
            ("post(handlers::", Method::POST),
            ("put(handlers::", Method::PUT),
            ("delete(handlers::", Method::DELETE),
        ] {
            if body.contains(needle) {
                routes.push((method, path.clone(), block_start < layer));
            }
        }
        offset = block_start + 1;
        rest = &ROUTES_SOURCE[offset..];
    }
    routes
}

/// Substitutes every `{param}` with a syntactically valid identifier.
fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for character in path.chars() {
        match character {
            '{' => {
                inside = true;
                out.push_str("00000000-0000-4000-8000-000000000000");
            }
            '}' => inside = false,
            _ if inside => {}
            _ => out.push(character),
        }
    }
    out
}

async fn router(demo_enabled: bool) -> Option<Router> {
    router_with(demo_enabled, false, chrono::Duration::seconds(90))
        .await
        .map(|(router, _)| router)
}

async fn router_with(
    demo_enabled: bool,
    open_registration: bool,
    max_snapshot_age: chrono::Duration,
) -> Option<(Router, AuthService)> {
    let Ok(database_url) = env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL is not set");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect");
    let config = AuthConfig {
        database_url,
        bind_address: "127.0.0.1:0".to_owned(),
        token_signing_seed: [7; 32],
        token_issuer: "perpeto-test".to_owned(),
        apple_bundle_id: "test.bundle".to_owned(),
        apple_web_services_id: None,
        google_client_ids: vec!["test.apps.googleusercontent.com".to_owned()],
        bootstrap_token_hash: None,
        bootstrap_token_expires_at: None,
        mfa_encryption_key: None,
        open_registration,
        passkey: None,
    };
    let service = AuthService::new(
        Repo::new(pool.clone()),
        &config,
        Arc::new(DevIdentityVerifier),
    );
    let strategies = StrategiesRepo::new(pool.clone());
    let demo = if demo_enabled {
        demo::seed(&service, &strategies).await.expect("seed demo");
        Some(Arc::new(DemoGate::new()))
    } else {
        None
    };
    Some((
        build_router(AppState {
            mode: TradingMode::Paper,
            service: service.clone(),
            passkeys: None,
            scanner: ScannerRepo::new(pool.clone()),
            execution: ExecutionService::default(),
            max_snapshot_age,
            positions: PositionsRepo::new(pool.clone()),
            breakers: BreakersRepo::new(pool.clone()),
            outbox: OutboxRepo::new(pool.clone()),
            strategies,
            monitoring: MonitoringRepo::new(pool.clone()),
            venues: VenueAccountsRepo::new(pool.clone()),
            connector_monitoring: ConnectorMonitoringRepo::new(pool.clone()),
            connector_registry: None,
            enrollment: None,
            connector_execution: ConnectorExecutionCoordinator::new(pool.clone()),
            reconciliation: ReconciliationService::new(pool),
            demo,
        }),
        service,
    ))
}

async fn call(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(
            request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn demo_token(router: &Router) -> String {
    let (status, body) = call(router, Method::POST, "/api/v1/demo/session", None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(
        body.get("refresh_token").is_none(),
        "demo sessions are access-only"
    );
    body["access_token"]
        .as_str()
        .expect("access token")
        .to_owned()
}

#[test]
fn the_route_table_parser_sees_every_mutating_route() {
    let mutating = declared_routes()
        .into_iter()
        .filter(|(method, _, protected)| *protected && *method != Method::GET)
        .count();
    // 43 protected mutations as of migration 0020; the count may only grow.
    assert!(mutating >= 43, "parsed {mutating} protected mutations");
}

#[tokio::test]
async fn every_protected_mutation_rejects_the_demo_principal() {
    let Some(router) = router(true).await else {
        return;
    };
    let token = demo_token(&router).await;
    let mut checked = 0;
    for (method, path, protected) in declared_routes() {
        if !protected || method == Method::GET {
            continue;
        }
        let (status, body) = call(&router, method.clone(), &concrete(&path), Some(&token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
        assert_eq!(body["code"], "DEMO_READ_ONLY", "{method} {path}");
        checked += 1;
    }
    assert!(checked >= 43, "checked {checked} mutations");
}

#[tokio::test]
async fn the_demo_principal_reads_the_public_demo_surfaces() {
    let Some(router) = router(true).await else {
        return;
    };
    let token = demo_token(&router).await;
    for path in [
        "/api/v1/auth/me",
        "/api/v1/opportunities",
        "/api/v1/risk/limits",
        "/api/v1/positions",
        "/api/v1/controls",
        "/api/v1/strategies",
        "/api/v1/portfolio",
        "/api/v1/health",
        "/api/v1/alerts",
        "/api/v1/venues",
    ] {
        let (status, body) = call(&router, Method::GET, path, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    }
    let (_, me) = call(&router, Method::GET, "/api/v1/auth/me", Some(&token)).await;
    assert_eq!(me["demo"], true);
    assert_eq!(me["roles"], serde_json::json!(["VIEWER"]));
    let (_, strategies) = call(&router, Method::GET, "/api/v1/strategies", Some(&token)).await;
    let seeded = strategies.as_array().expect("strategy list");
    assert!(seeded.iter().any(|strategy| strategy["enabled"] == true));
}

#[tokio::test]
async fn the_demo_endpoint_is_absent_unless_enabled() {
    let Some(router) = router(false).await else {
        return;
    };
    let (status, _) = call(&router, Method::POST, "/api/v1/demo/session", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Signs in a fresh open-registration user (OWNER, TRADER, APPROVER) through
/// the development verifier and returns its access token.
async fn owner_token(service: &AuthService) -> String {
    let verifier = "verifier-000000000000000000000000000000000000000";
    let challenge = service
        .create_challenge(CreateChallengeRequest {
            provider: Provider::Google,
            redirect_uri: "signex://auth/callback".to_owned(),
            code_challenge: encode_base64url(&sha256(verifier.as_bytes())),
            purpose: "SIGN_IN".to_owned(),
        })
        .await
        .expect("challenge");
    let transition = service
        .exchange(ExchangeRequest {
            challenge_id: challenge.id,
            authorization_code: "auth-code".to_owned(),
            identity_token: format!("perpeto-dev:freshness-{}", uuid::Uuid::new_v4()),
            code_verifier: Some(verifier.to_owned()),
            apple_profile: None,
            device: DeviceRegistration {
                installation_id: uuid::Uuid::new_v4().to_string(),
                platform: "IOS".to_owned(),
                app_version: "test".to_owned(),
                display_name: "Test".to_owned(),
            },
        })
        .await
        .expect("exchange");
    transition.session.expect("session").access_token
}

/// DEC-0016: a paper open fills against the latest stored snapshot only while
/// it is fresh; a stalled engine must not leave the API filling old books.
#[tokio::test]
async fn a_stale_market_snapshot_refuses_a_paper_entry() {
    let Some((router, service)) = router_with(false, true, chrono::Duration::seconds(1)).await
    else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&env::var("DATABASE_URL").expect("database url"))
        .await
        .expect("connect");
    sqlx::query(
        "INSERT INTO scan_runs (id, scanned_at, model_version, market_snapshot, created_at) \
         VALUES ($1, now(), 'freshness-test', $2, now() - interval '10 minutes')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(r#"{"taken_at":"2026-11-24T00:00:00Z","legs":[],"books":[],"venues":[]}"#)
    .execute(&pool)
    .await
    .expect("stale snapshot");
    let token = owner_token(&service).await;
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/positions")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"route_id":"PERP_PERP:BINANCE>BYBIT:BTC"}"#))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.expect("body"))
            .expect("problem");
    assert_eq!(body["code"], "STALE_MARKET_DATA");
}

/// DEC-0021: a session stops working the moment it is logged out, not when its
/// 15-minute access token expires.
#[tokio::test]
async fn a_logged_out_access_token_is_refused_at_once() {
    let Some((router, service)) = router_with(false, true, chrono::Duration::seconds(90)).await
    else {
        return;
    };
    let token = owner_token(&service).await;
    let (status, _) = call(&router, Method::GET, "/api/v1/auth/me", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&router, Method::POST, "/api/v1/auth/logout", Some(&token)).await;
    assert!(status.is_success(), "logout returned {status}");
    let (status, body) = call(&router, Method::GET, "/api/v1/auth/me", Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "SESSION_REVOKED");
}
