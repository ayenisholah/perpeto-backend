//! Postgres round trips for the market snapshot stored with each scan
//! (migration 0019, DEC-0016).
//!
//! Skipped unless `DATABASE_URL` is set. CI applies the migrations as the schema
//! owner and runs this as the non-owning runtime role, so it also proves that
//! role can write and read the new column.

use chrono::{DateTime, TimeZone, Utc};
use funding_arb_domain::market::Venue;
use funding_arb_scanner_service::fixtures;
use funding_arb_scanner_service::{
    BookSide, FeedStatus, LegBook, MarketSnapshot, Scanner, ScannerRepo, VenueFeedStatus,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;

// Every scenario reads "the latest snapshot", so serialize them.
static TEST_LOCK: Mutex<()> = Mutex::const_new(());

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 19, hour, 0, 0)
        .single()
        .expect("valid time")
}

/// A fixture snapshot enriched with a book and feed reports, using decimals
/// beyond f64 precision so any lossy step would show.
fn snapshot(now: DateTime<Utc>, marker: &str) -> MarketSnapshot {
    let mut snapshot =
        MarketSnapshot::from_legs(now, fixtures::load_at(now).expect("fixture legs"));
    let leg = &snapshot.legs[0].observation.instrument;
    snapshot.books.push(LegBook {
        venue: leg.venue,
        symbol: leg.symbol.clone(),
        contract_multiplier: Decimal::ONE,
        bids: BookSide {
            levels: vec![(
                "97120.123456789012345".parse().expect("decimal"),
                Decimal::TWO,
            )],
        },
        asks: BookSide {
            levels: vec![(
                "97120.223456789012345".parse().expect("decimal"),
                Decimal::ONE,
            )],
        },
    });
    snapshot.venues.push(VenueFeedStatus {
        venue: Venue::Bybit,
        status: FeedStatus::Unavailable,
        observed_at: None,
        error: Some(marker.to_owned()),
    });
    snapshot
}

#[tokio::test]
async fn the_latest_snapshot_round_trips_exactly() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = TEST_LOCK.lock().await;
    let repo = ScannerRepo::new(pool);
    let scanner = Scanner::default();
    let now = at(0);
    let stored = snapshot(now, "round-trip");
    let result = scanner.scan_legs(now, &stored.legs);

    let before = Utc::now();
    repo.record_scan_with_snapshot(&result, Some(&stored))
        .await
        .expect("record scan");
    let latest = repo
        .latest_snapshot()
        .await
        .expect("read snapshot")
        .expect("a snapshot exists");

    assert!(latest.recorded_at >= before - chrono::Duration::seconds(5));
    assert_eq!(latest.snapshot.taken_at, now);
    assert_eq!(latest.snapshot.legs.len(), stored.legs.len());
    assert_eq!(
        latest.snapshot.legs[0].observation.funding_rate,
        stored.legs[0].observation.funding_rate
    );
    assert_eq!(latest.snapshot.books, stored.books);
    assert_eq!(latest.snapshot.venues, stored.venues);
}

#[tokio::test]
async fn a_newer_snapshot_wins_and_a_scan_without_one_does_not_hide_it() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _guard = TEST_LOCK.lock().await;
    let repo = ScannerRepo::new(pool);
    let scanner = Scanner::default();

    let first = snapshot(at(4), "first");
    repo.record_scan_with_snapshot(&scanner.scan_legs(at(4), &first.legs), Some(&first))
        .await
        .expect("record first");
    let second = snapshot(at(8), "second");
    repo.record_scan_with_snapshot(&scanner.scan_legs(at(8), &second.legs), Some(&second))
        .await
        .expect("record second");
    // A legacy scan with no snapshot must not become "the latest snapshot".
    repo.record_scan(&scanner.scan(at(12)).expect("fixture scan"))
        .await
        .expect("record bare scan");

    let latest = repo
        .latest_snapshot()
        .await
        .expect("read snapshot")
        .expect("a snapshot exists");
    assert_eq!(latest.snapshot.venues[0].error.as_deref(), Some("second"));
    assert_eq!(latest.snapshot.taken_at, at(8));
}
