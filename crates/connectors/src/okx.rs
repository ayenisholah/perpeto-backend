//! OKX V5 spot and USDT perpetual adapter with demo-trading isolation.

use std::{str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use funding_arb_domain::market::Venue;
use futures_util::{StreamExt, stream};
use hmac::{Hmac, Mac};
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::ws::{JsonWsConfig, connect_json};
use crate::{
    AccountId, Balance, CancelAck, CancelRequest, CanonicalOrderIntent, ConnectorCredentials,
    ConnectorError, ConnectorErrorKind, ConnectorResult, FeeSchedule, Fill, FillPage,
    FundingLedgerEntry, FundingQuote, HealthStatus, Instrument, InstrumentId, MarketEvent,
    MarketEventStream, MarketSubscription, OrderAck, OrderLookup, OrderSide, OrderStatus,
    PreparedOrder, PriceLevel, PrivateEvent, PrivateEventStream, ProductType, TimeRange,
    VenueCapabilities, VenueConnector, VenueEnvironment, VenueHealth, VenueOrder, VenuePosition,
    VenueTimestamp,
};

#[derive(Clone)]
pub struct OkxConnector {
    environment: VenueEnvironment,
    base_url: Arc<str>,
    client: reqwest::Client,
    credentials: Option<Arc<ConnectorCredentials>>,
    instruments: Arc<RwLock<Vec<Instrument>>>,
}

impl OkxConnector {
    #[must_use]
    pub fn new(environment: VenueEnvironment, credentials: Option<ConnectorCredentials>) -> Self {
        Self::with_base_url(environment, credentials, "https://www.okx.com")
    }

    /// Regional OKX accounts can select the documented regional REST origin.
    #[must_use]
    pub fn with_base_url(
        environment: VenueEnvironment,
        credentials: Option<ConnectorCredentials>,
        base_url: &str,
    ) -> Self {
        Self {
            environment,
            base_url: Arc::from(base_url.trim_end_matches('/')),
            client: reqwest::Client::new(),
            credentials: credentials.map(Arc::new),
            instruments: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn public_get(&self, path: &str, query: &[(&str, String)]) -> ConnectorResult<Value> {
        self.send(Method::GET, path, query, None, false).await
    }

    async fn private_get(&self, path: &str, query: &[(&str, String)]) -> ConnectorResult<Value> {
        self.send(Method::GET, path, query, None, true).await
    }

    async fn private_post(&self, path: &str, body: &Value) -> ConnectorResult<Value> {
        self.send(Method::POST, path, &[], Some(body), true).await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        authenticated: bool,
    ) -> ConnectorResult<Value> {
        let query = serde_urlencoded::to_string(query).map_err(|_| invalid("QUERY_ENCODING"))?;
        let request_path = if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{query}")
        };
        let url = format!("{}{request_path}", self.base_url);
        let encoded_body = body
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| invalid("BODY_ENCODING"))?
            .unwrap_or_default();
        let mut request = self.client.request(method.clone(), url);
        if !encoded_body.is_empty() {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(encoded_body.clone());
        }
        if authenticated {
            let credentials = self.credentials.as_ref().ok_or_else(|| ConnectorError {
                kind: ConnectorErrorKind::FatalConfiguration,
                code: "OKX_CREDENTIALS_MISSING".into(),
                message: "OKX credentials are not configured".into(),
                retry_after_ms: None,
            })?;
            let passphrase = credentials
                .passphrase
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ConnectorError {
                    kind: ConnectorErrorKind::FatalConfiguration,
                    code: "OKX_PASSPHRASE_MISSING".into(),
                    message: "OKX passphrase is not configured".into(),
                    retry_after_ms: None,
                })?;
            let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            let signable = format!("{timestamp}{}{request_path}{encoded_body}", method.as_str());
            let signature = hmac_base64(credentials.api_secret.as_bytes(), signable.as_bytes())?;
            request = request
                .header("OK-ACCESS-KEY", &credentials.api_key)
                .header("OK-ACCESS-SIGN", signature)
                .header("OK-ACCESS-TIMESTAMP", timestamp)
                .header("OK-ACCESS-PASSPHRASE", passphrase);
            if self.environment == VenueEnvironment::Demo {
                request = request.header("x-simulated-trading", "1");
            }
        }
        let response = request.send().await.map_err(|_| ConnectorError {
            kind: if method == Method::POST {
                ConnectorErrorKind::UnknownOutcome
            } else {
                ConnectorErrorKind::Transient
            },
            code: "OKX_NETWORK".into(),
            message: "OKX network request failed".into(),
            retry_after_ms: None,
        })?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| invalid("INVALID_JSON"))?;
        if status.as_u16() == 429 {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::RateLimited,
                code: "OKX_RATE_LIMITED".into(),
                message: "OKX rate limit reached".into(),
                retry_after_ms: None,
            });
        }
        if status.is_server_error() {
            return Err(ConnectorError {
                kind: if method == Method::POST {
                    ConnectorErrorKind::UnknownOutcome
                } else {
                    ConnectorErrorKind::Transient
                },
                code: format!("OKX_HTTP_{}", status.as_u16()),
                message: "OKX server error".into(),
                retry_after_ms: None,
            });
        }
        let code = value.get("code").and_then(Value::as_str).unwrap_or("-1");
        if !status.is_success() || code != "0" {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::InvalidResponse,
                code: format!("OKX_{code}"),
                message: value
                    .get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("OKX request failed")
                    .into(),
                retry_after_ms: None,
            });
        }
        Ok(value)
    }

    async fn instruments_for(&self, product: ProductType) -> ConnectorResult<Vec<Instrument>> {
        let response = self
            .public_get(
                "/api/v5/public/instruments",
                &[("instType", instrument_type(product).into())],
            )
            .await?;
        let mut instruments = Vec::new();
        for item in data(&response)? {
            let symbol = text(item, "instId")?;
            let accepted = match product {
                ProductType::Spot => matches!(symbol, "BTC-USDT" | "ETH-USDT"),
                ProductType::LinearPerpetual => {
                    matches!(symbol, "BTC-USDT-SWAP" | "ETH-USDT-SWAP")
                }
            };
            if !accepted {
                continue;
            }
            let multiplier = if product == ProductType::Spot {
                Decimal::ONE
            } else {
                decimal(item, "ctVal")?
            };
            instruments.push(Instrument {
                id: instrument_id(symbol, product),
                canonical_underlying: symbol.split('-').next().unwrap_or_default().into(),
                quote_asset: "USDT".into(),
                settlement_asset: item
                    .get("settleCcy")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("USDT")
                    .into(),
                margin_asset: "USDT".into(),
                contract_multiplier: multiplier,
                price_tick: decimal(item, "tickSz")?,
                quantity_step: decimal(item, "lotSz")?,
                minimum_quantity: decimal(item, "minSz")?,
                maximum_quantity: optional_decimal(item, "maxLmtSz")?,
                minimum_notional: Decimal::ZERO,
                funding_interval_hours: (product == ProductType::LinearPerpetual).then_some(8),
                funding_cap: None,
                funding_floor: None,
                maximum_leverage: optional_decimal(item, "lever")?,
                trading: text(item, "state")? == "live",
                metadata_version: format!("okx-v5-{}", Utc::now().date_naive()),
                venue_updated_at: item
                    .get("listTime")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|value| millis(value).ok()),
                received_at: Utc::now(),
            });
        }
        Ok(instruments)
    }

    // Implemented but not yet wired into the WebSocket stream. Binance calls its
    // equivalent on a transport reconnect to rebuild the book and re-anchor the
    // sequence; OKX's reconnect path does not, so it currently recovers only
    // through its own checksum/sequence handling. Wiring this needs credentialed
    // venue evidence and belongs to the open M3 certification work.
    #[expect(dead_code, reason = "pending M3 reconnect snapshot bridge")]
    async fn book_snapshot(
        &self,
        subscription: &MarketSubscription,
    ) -> ConnectorResult<MarketEvent> {
        let response = self
            .public_get(
                "/api/v5/market/books",
                &[
                    ("instId", subscription.instrument.symbol.clone()),
                    ("sz", subscription.depth.clamp(1, 400).to_string()),
                ],
            )
            .await?;
        let item = data(&response)?
            .first()
            .ok_or_else(|| invalid("BOOK_EMPTY"))?;
        let received_at = Utc::now();
        let venue_time = millis(i64_text(item, "ts")?)?;
        Ok(MarketEvent::BookSnapshot {
            instrument: subscription.instrument.clone(),
            sequence: item
                .get("seqId")
                .and_then(Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or_else(|| {
                    u64::try_from(venue_time.timestamp_millis()).unwrap_or_default()
                }),
            bids: levels(item, "bids")?,
            asks: levels(item, "asks")?,
            venue_time,
            received_at,
            checksum: item.get("checksum").map(value_to_string).transpose()?,
        })
    }

    async fn orders(&self, account: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
        let _ = account;
        let response = self
            .private_get("/api/v5/trade/orders-pending", &[])
            .await?;
        let mut orders = Vec::new();
        for item in data(&response)? {
            orders.push(self.normalize_order(item).await?);
        }
        Ok(orders)
    }

    async fn normalize_order(&self, item: &Value) -> ConnectorResult<VenueOrder> {
        let mut order = parse_order(item)?;
        if order.instrument.product == ProductType::LinearPerpetual {
            let multiplier = self
                .instruments
                .read()
                .await
                .iter()
                .find(|value| value.id == order.instrument)
                .map(|value| value.contract_multiplier)
                .unwrap_or(Decimal::ONE);
            order.requested_quantity *= multiplier;
            order.filled_quantity *= multiplier;
        }
        Ok(order)
    }
}

#[async_trait]
impl VenueConnector for OkxConnector {
    fn venue(&self) -> Venue {
        Venue::Okx
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            products: vec![ProductType::Spot, ProductType::LinearPerpetual],
            market_stream: true,
            private_stream: self.credentials.is_some(),
            funding_history: true,
            order_submission: self.credentials.is_some()
                && self.environment == VenueEnvironment::Demo,
            borrow: false,
            internal_transfer: false,
        }
    }

    async fn health(&self) -> ConnectorResult<VenueHealth> {
        let timestamp = self.server_time().await?;
        let observed_at = Utc::now();
        let drift = (observed_at - timestamp.venue_time).num_milliseconds();
        Ok(VenueHealth {
            venue: Venue::Okx,
            environment: self.environment,
            status: if drift.unsigned_abs() <= 2_000 {
                HealthStatus::Healthy
            } else {
                HealthStatus::Degraded
            },
            server_time: timestamp.venue_time,
            observed_at,
            clock_drift_ms: drift,
            rate_limit_remaining: None,
            detail: None,
        })
    }

    async fn server_time(&self) -> ConnectorResult<VenueTimestamp> {
        let response = self.public_get("/api/v5/public/time", &[]).await?;
        let item = data(&response)?
            .first()
            .ok_or_else(|| invalid("SERVER_TIME_EMPTY"))?;
        Ok(VenueTimestamp {
            venue_time: millis(i64_text(item, "ts")?)?,
            received_at: Utc::now(),
        })
    }

    async fn instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        let mut instruments = self.instruments_for(ProductType::Spot).await?;
        instruments.extend(self.instruments_for(ProductType::LinearPerpetual).await?);
        *self.instruments.write().await = instruments.clone();
        Ok(instruments)
    }

    async fn market_stream(
        &self,
        subscriptions: Vec<MarketSubscription>,
    ) -> ConnectorResult<MarketEventStream> {
        if subscriptions.is_empty() {
            return Err(invalid("EMPTY_SUBSCRIPTION"));
        }
        let args = subscriptions
            .iter()
            .map(|subscription| {
                json!({"channel": "books", "instId": subscription.instrument.symbol})
            })
            .collect::<Vec<_>>();
        let stream_subscriptions = Arc::new(subscriptions);
        let frames = connect_json(JsonWsConfig {
            name: "OKX",
            url: "wss://ws.okx.com:8443/ws/v5/public".into(),
            initial_messages: vec![json!({"op": "subscribe", "args": args})],
            heartbeat: Some("ping".into()),
            inactivity_timeout: Duration::from_secs(20),
        });
        Ok(Box::pin(frames.filter_map(move |frame| {
            let subscriptions = stream_subscriptions.clone();
            async move {
                match frame {
                    Err(error) => Some(Err(error)),
                    Ok(frame) => match parse_market_frame(&frame, &subscriptions) {
                        Ok(Some(event)) => Some(Ok(event)),
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    },
                }
            }
        })))
    }

    async fn private_stream(&self, account: &AccountId) -> ConnectorResult<PrivateEventStream> {
        let _ = account;
        let credentials = self.credentials.as_ref().ok_or_else(|| ConnectorError {
            kind: ConnectorErrorKind::FatalConfiguration,
            code: "OKX_CREDENTIALS_MISSING".into(),
            message: "OKX credentials are not configured".into(),
            retry_after_ms: None,
        })?;
        let passphrase = credentials
            .passphrase
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("PASSPHRASE_MISSING"))?;
        let timestamp = Utc::now().timestamp().to_string();
        let signature = hmac_base64(
            credentials.api_secret.as_bytes(),
            format!("{timestamp}GET/users/self/verify").as_bytes(),
        )?;
        let frames = connect_json(JsonWsConfig {
            name: "OKX_PRIVATE",
            url: okx_private_ws_url(self.environment).to_owned(),
            initial_messages: vec![
                json!({"op": "login", "args": [{"apiKey": credentials.api_key.clone(), "passphrase": passphrase, "timestamp": timestamp, "sign": signature}]}),
                json!({"op": "subscribe", "args": [
                    {"channel": "orders", "instType": "ANY"},
                    {"channel": "account"},
                    {"channel": "positions", "instType": "ANY"}
                ]}),
            ],
            heartbeat: Some("ping".into()),
            inactivity_timeout: Duration::from_secs(20),
        });
        Ok(Box::pin(frames.flat_map(|frame| {
            let events = match frame {
                Err(error) => vec![Err(error)],
                Ok(frame) => match parse_private_frame(&frame) {
                    Ok(events) => events.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                },
            };
            stream::iter(events)
        })))
    }

    async fn fee_schedule(&self, _account: &AccountId) -> ConnectorResult<Vec<FeeSchedule>> {
        let mut fees = Vec::new();
        for product in [ProductType::Spot, ProductType::LinearPerpetual] {
            let response = self
                .private_get(
                    "/api/v5/account/trade-fee",
                    &[("instType", instrument_type(product).into())],
                )
                .await?;
            let item = data(&response)?
                .first()
                .ok_or_else(|| invalid("FEE_EMPTY"))?;
            fees.push(FeeSchedule {
                product,
                maker: decimal(item, "maker")?.abs(),
                taker: decimal(item, "taker")?.abs(),
                effective_at: Utc::now(),
            });
        }
        Ok(fees)
    }

    async fn current_funding(&self, instrument: &InstrumentId) -> ConnectorResult<FundingQuote> {
        if instrument.product != ProductType::LinearPerpetual {
            return Err(unsupported("FUNDING_SPOT_UNSUPPORTED"));
        }
        let funding = self
            .public_get(
                "/api/v5/public/funding-rate",
                &[("instId", instrument.symbol.clone())],
            )
            .await?;
        let ticker = self
            .public_get(
                "/api/v5/market/ticker",
                &[("instId", instrument.symbol.clone())],
            )
            .await?;
        let funding = data(&funding)?
            .first()
            .ok_or_else(|| invalid("FUNDING_EMPTY"))?;
        let ticker = data(&ticker)?
            .first()
            .ok_or_else(|| invalid("TICKER_EMPTY"))?;
        let price = decimal(ticker, "last")?;
        Ok(FundingQuote {
            instrument: instrument.clone(),
            rate: decimal(funding, "fundingRate")?,
            interval_hours: funding_interval(funding).unwrap_or(8),
            mark_price: price,
            index_price: price,
            next_funding_at: millis(i64_text(funding, "nextFundingTime")?)?,
            observed_at: Utc::now(),
        })
    }

    async fn funding_history(
        &self,
        instrument: &InstrumentId,
        range: &TimeRange,
    ) -> ConnectorResult<Vec<FundingQuote>> {
        let response = self
            .public_get(
                "/api/v5/public/funding-rate-history",
                &[
                    ("instId", instrument.symbol.clone()),
                    ("before", range.end.timestamp_millis().to_string()),
                    ("after", range.start.timestamp_millis().to_string()),
                    ("limit", "100".into()),
                ],
            )
            .await?;
        data(&response)?
            .iter()
            .map(|item| {
                Ok(FundingQuote {
                    instrument: instrument.clone(),
                    rate: decimal(item, "fundingRate")?,
                    interval_hours: 8,
                    mark_price: Decimal::ZERO,
                    index_price: Decimal::ZERO,
                    next_funding_at: millis(i64_text(item, "fundingTime")?)?,
                    observed_at: Utc::now(),
                })
            })
            .collect()
    }

    async fn balances(&self, _account: &AccountId) -> ConnectorResult<Vec<Balance>> {
        let response = self.private_get("/api/v5/account/balance", &[]).await?;
        let mut balances = Vec::new();
        for account in data(&response)? {
            for item in array(account, "details")? {
                let total = optional_decimal(item, "cashBal")?.unwrap_or(Decimal::ZERO);
                if total.is_zero() {
                    continue;
                }
                let available = optional_decimal(item, "availBal")?.unwrap_or(Decimal::ZERO);
                balances.push(Balance {
                    asset: text(item, "ccy")?.into(),
                    total,
                    available,
                    locked: total - available,
                    venue_updated_at: account
                        .get("uTime")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|value| millis(value).ok()),
                    received_at: Utc::now(),
                });
            }
        }
        Ok(balances)
    }

    async fn positions(&self, _account: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
        let response = self
            .private_get("/api/v5/account/positions", &[("instType", "SWAP".into())])
            .await?;
        let mut positions = Vec::new();
        for item in data(&response)? {
            let contracts = decimal(item, "pos")?;
            if contracts.is_zero() {
                continue;
            }
            let symbol = text(item, "instId")?;
            let multiplier = self
                .instruments
                .read()
                .await
                .iter()
                .find(|value| value.id.symbol == symbol)
                .map(|value| value.contract_multiplier)
                .unwrap_or(Decimal::ONE);
            positions.push(VenuePosition {
                instrument: instrument_id(symbol, ProductType::LinearPerpetual),
                quantity: contracts * multiplier,
                entry_price: decimal(item, "avgPx")?,
                mark_price: decimal(item, "markPx")?,
                leverage: decimal(item, "lever")?,
                liquidation_price: optional_decimal(item, "liqPx")?,
                venue_updated_at: item
                    .get("uTime")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|value| millis(value).ok()),
                received_at: Utc::now(),
            });
        }
        Ok(positions)
    }

    async fn open_orders(&self, account: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
        self.orders(account).await
    }

    async fn fills(
        &self,
        _account: &AccountId,
        _cursor: Option<&str>,
    ) -> ConnectorResult<FillPage> {
        let mut fills = Vec::new();
        for (kind, product) in [
            ("SPOT", ProductType::Spot),
            ("SWAP", ProductType::LinearPerpetual),
        ] {
            let response = self
                .private_get("/api/v5/trade/fills-history", &[("instType", kind.into())])
                .await?;
            for item in data(&response)? {
                let symbol = text(item, "instId")?;
                let multiplier = if product == ProductType::Spot {
                    Decimal::ONE
                } else {
                    self.instruments
                        .read()
                        .await
                        .iter()
                        .find(|value| value.id.symbol == symbol)
                        .map(|value| value.contract_multiplier)
                        .unwrap_or(Decimal::ONE)
                };
                fills.push(Fill {
                    venue_fill_id: text(item, "tradeId")?.into(),
                    venue_order_id: text(item, "ordId")?.into(),
                    instrument: instrument_id(symbol, product),
                    quantity: decimal(item, "fillSz")? * multiplier,
                    price: decimal(item, "fillPx")?,
                    fee: decimal(item, "fee")?.abs(),
                    fee_asset: text(item, "feeCcy")?.into(),
                    occurred_at: millis(i64_text(item, "ts")?)?,
                    received_at: Utc::now(),
                });
            }
        }
        Ok(FillPage {
            fills,
            next_cursor: None,
        })
    }

    async fn funding_ledger(
        &self,
        _account: &AccountId,
        range: &TimeRange,
    ) -> ConnectorResult<Vec<FundingLedgerEntry>> {
        let response = self
            .private_get(
                "/api/v5/account/bills-archive",
                &[
                    ("instType", "SWAP".into()),
                    ("type", "8".into()),
                    ("begin", range.start.timestamp_millis().to_string()),
                    ("end", range.end.timestamp_millis().to_string()),
                    ("limit", "100".into()),
                ],
            )
            .await?;
        data(&response)?
            .iter()
            .map(|item| {
                let symbol = text(item, "instId")?;
                Ok(FundingLedgerEntry {
                    venue_entry_id: text(item, "billId")?.into(),
                    instrument: instrument_id(symbol, ProductType::LinearPerpetual),
                    amount: decimal(item, "balChg")?,
                    asset: text(item, "ccy")?.into(),
                    occurred_at: millis(i64_text(item, "ts")?)?,
                })
            })
            .collect()
    }

    async fn prepare_order(&self, order: &CanonicalOrderIntent) -> ConnectorResult<PreparedOrder> {
        if order.instrument.venue != Venue::Okx {
            return Err(invalid("VENUE_MISMATCH"));
        }
        let instruments = self.instruments.read().await;
        let metadata = instruments
            .iter()
            .find(|value| value.id == order.instrument)
            .ok_or_else(|| invalid("INSTRUMENT_METADATA_REQUIRED"))?;
        let multiplier = metadata.contract_multiplier;
        let quantity_step = metadata.quantity_step;
        let native_quantity = order.base_quantity / multiplier;
        if (native_quantity / quantity_step).fract() != Decimal::ZERO {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::FatalConfiguration,
                code: "OKX_QUANTITY_NOT_REPRESENTABLE".into(),
                message: "base quantity cannot be represented on the OKX lot step".into(),
                retry_after_ms: None,
            });
        }
        let mut payload = json!({
            "instId": order.instrument.symbol,
            "tdMode": if order.instrument.product == ProductType::Spot { "cash" } else { "isolated" },
            "side": side_wire(order.side),
            "ordType": "limit",
            "sz": native_quantity.to_string(),
            "px": order.limit_price.to_string(),
            "clOrdId": order.client_order_id,
            "reduceOnly": order.reduce_only,
        });
        if order.instrument.product == ProductType::LinearPerpetual {
            payload["posSide"] = json!("net");
        }
        let encoded = serde_json::to_vec(&payload).map_err(|_| invalid("BODY_ENCODING"))?;
        Ok(PreparedOrder {
            intent: order.clone(),
            native_symbol: order.instrument.symbol.clone(),
            native_quantity,
            equivalent_base_quantity: native_quantity * multiplier,
            native_price: order.limit_price,
            projected_delta_usd: native_quantity * multiplier * order.limit_price,
            payload,
            payload_hash: hex(&Sha256::digest(encoded)),
        })
    }

    async fn submit_order(&self, order: &PreparedOrder) -> ConnectorResult<OrderAck> {
        if self.environment != VenueEnvironment::Demo {
            return Err(unsupported("PRODUCTION_WRITE_FORBIDDEN"));
        }
        let response = self
            .private_post("/api/v5/trade/order", &order.payload)
            .await?;
        let item = data(&response)?
            .first()
            .ok_or_else(|| invalid("ORDER_ACK_EMPTY"))?;
        let code = item.get("sCode").and_then(Value::as_str).unwrap_or("-1");
        if code != "0" {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::InvalidResponse,
                code: format!("OKX_{code}"),
                message: item
                    .get("sMsg")
                    .and_then(Value::as_str)
                    .unwrap_or("OKX order rejected")
                    .into(),
                retry_after_ms: None,
            });
        }
        Ok(OrderAck {
            venue_order_id: item.get("ordId").and_then(Value::as_str).map(str::to_owned),
            client_order_id: order.intent.client_order_id.clone(),
            status: OrderStatus::Pending,
            acknowledged_at: Utc::now(),
        })
    }

    async fn cancel_order(&self, request: &CancelRequest) -> ConnectorResult<CancelAck> {
        if self.environment != VenueEnvironment::Demo {
            return Err(unsupported("PRODUCTION_WRITE_FORBIDDEN"));
        }
        self.private_post(
            "/api/v5/trade/cancel-order",
            &json!({
                "instId": request.instrument.symbol,
                "ordId": request.venue_order_id,
                "clOrdId": request.client_order_id,
            }),
        )
        .await?;
        Ok(CancelAck {
            status: OrderStatus::Pending,
            acknowledged_at: Utc::now(),
        })
    }

    async fn query_order(
        &self,
        _account: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>> {
        for symbol in ["BTC-USDT", "ETH-USDT", "BTC-USDT-SWAP", "ETH-USDT-SWAP"] {
            let lookup = match key {
                OrderLookup::VenueOrderId(id) => ("ordId", id.clone()),
                OrderLookup::ClientOrderId(id) => ("clOrdId", id.clone()),
            };
            match self
                .private_get("/api/v5/trade/order", &[("instId", symbol.into()), lookup])
                .await
            {
                Ok(response) => {
                    if let Some(item) = data(&response)?.first() {
                        return self.normalize_order(item).await.map(Some);
                    }
                }
                Err(error) if error.code == "OKX_51603" => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
}

fn parse_market_frame(
    frame: &Value,
    subscriptions: &[MarketSubscription],
) -> ConnectorResult<Option<MarketEvent>> {
    if frame.get("event").is_some() || frame.get("_perpeto_transport").is_some() {
        return Ok(None);
    }
    let argument = frame
        .get("arg")
        .ok_or_else(|| invalid("WS_ARGUMENT_MISSING"))?;
    if argument.get("channel").and_then(Value::as_str) != Some("books") {
        return Ok(None);
    }
    let symbol = text(argument, "instId")?;
    let subscription = subscriptions
        .iter()
        .find(|subscription| subscription.instrument.symbol == symbol)
        .ok_or_else(|| invalid("WS_UNSUBSCRIBED_SYMBOL"))?;
    let item = data(frame)?
        .first()
        .ok_or_else(|| invalid("WS_DATA_EMPTY"))?;
    let sequence = i64_text(item, "seqId")?;
    if sequence < 0 {
        return Err(reconciliation("SEQUENCE_NEGATIVE"));
    }
    let sequence = u64::try_from(sequence).map_err(|_| invalid("WS_SEQUENCE_INVALID"))?;
    let venue_time = millis(i64_text(item, "ts")?)?;
    let checksum = item.get("checksum").map(value_to_string).transpose()?;
    match frame.get("action").and_then(Value::as_str) {
        Some("snapshot") => Ok(Some(MarketEvent::BookSnapshot {
            instrument: subscription.instrument.clone(),
            sequence,
            bids: levels(item, "bids")?,
            asks: levels(item, "asks")?,
            venue_time,
            received_at: Utc::now(),
            checksum,
        })),
        Some("update") => {
            let previous = i64_text(item, "prevSeqId")?;
            if previous < 0 {
                return Err(reconciliation("PREVIOUS_SEQUENCE_NEGATIVE"));
            }
            let previous =
                u64::try_from(previous).map_err(|_| invalid("WS_PREVIOUS_SEQUENCE_INVALID"))?;
            // OKX documents a maintenance reset as an update whose `seqId` is
            // smaller than its `prevSeqId`, after which sequencing restarts in a
            // new range. A local book keyed on the old range would treat every
            // later update as a duplicate and silently freeze, so fail closed:
            // the consumer must discard the book and resubscribe for a fresh
            // snapshot. The REST book carries no `seqId` and cannot re-seed it.
            if sequence < previous {
                return Err(reconciliation("SEQUENCE_RESET"));
            }
            Ok(Some(MarketEvent::BookDelta {
                instrument: subscription.instrument.clone(),
                first_sequence: previous.saturating_add(1),
                last_sequence: sequence,
                bids: levels(item, "bids")?,
                asks: levels(item, "asks")?,
                venue_time,
                received_at: Utc::now(),
                checksum,
            }))
        }
        _ => Err(invalid("WS_ACTION_INVALID")),
    }
}

fn okx_private_ws_url(environment: VenueEnvironment) -> &'static str {
    if environment == VenueEnvironment::Demo {
        "wss://wspap.okx.com:8443/ws/v5/private"
    } else {
        "wss://ws.okx.com:8443/ws/v5/private"
    }
}

fn parse_private_frame(frame: &Value) -> ConnectorResult<Vec<PrivateEvent>> {
    if frame.get("_perpeto_transport").is_some() {
        return Ok(vec![PrivateEvent::Heartbeat(Utc::now())]);
    }
    if frame.get("event").is_some() {
        if frame.get("event").and_then(Value::as_str) == Some("error") {
            return Err(reconciliation("PRIVATE_SUBSCRIPTION_FAILED"));
        }
        return Ok(Vec::new());
    }
    let argument = frame
        .get("arg")
        .ok_or_else(|| invalid("PRIVATE_ARGUMENT_MISSING"))?;
    let channel = text(argument, "channel")?;
    let mut events = Vec::new();
    for item in data(frame)? {
        if channel == "orders" {
            let order = parse_order(item)?;
            let trade_id = item
                .get("tradeId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let fill_quantity = optional_decimal(item, "fillSz")?.unwrap_or(Decimal::ZERO);
            events.push(PrivateEvent::Order(order.clone()));
            if !trade_id.is_empty() && fill_quantity > Decimal::ZERO {
                events.push(PrivateEvent::Fill(Fill {
                    venue_fill_id: trade_id.into(),
                    venue_order_id: order.venue_order_id,
                    instrument: order.instrument,
                    quantity: fill_quantity,
                    price: optional_decimal(item, "fillPx")?.unwrap_or(Decimal::ZERO),
                    fee: optional_decimal(item, "fee")?
                        .unwrap_or(Decimal::ZERO)
                        .abs(),
                    fee_asset: item
                        .get("feeCcy")
                        .and_then(Value::as_str)
                        .unwrap_or("UNKNOWN")
                        .into(),
                    occurred_at: item
                        .get("fillTime")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|value| millis(value).ok())
                        .unwrap_or_else(Utc::now),
                    received_at: Utc::now(),
                }));
            }
        } else if channel == "account" {
            for detail in item
                .get("details")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let total = optional_decimal(detail, "eq")?.unwrap_or(Decimal::ZERO);
                let available = optional_decimal(detail, "availEq")?
                    .or(optional_decimal(detail, "availBal")?)
                    .unwrap_or(total);
                events.push(PrivateEvent::Balance(Balance {
                    asset: text(detail, "ccy")?.into(),
                    total,
                    available,
                    locked: (total - available).max(Decimal::ZERO),
                    venue_updated_at: item
                        .get("uTime")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|value| millis(value).ok()),
                    received_at: Utc::now(),
                }));
            }
        } else if channel == "positions" {
            events.push(PrivateEvent::Position(VenuePosition {
                instrument: instrument_id(
                    text(item, "instId")?,
                    product_from_symbol(text(item, "instId")?),
                ),
                quantity: decimal(item, "pos")?,
                entry_price: optional_decimal(item, "avgPx")?.unwrap_or(Decimal::ZERO),
                mark_price: optional_decimal(item, "markPx")?.unwrap_or(Decimal::ZERO),
                leverage: optional_decimal(item, "lever")?.unwrap_or(Decimal::ONE),
                liquidation_price: optional_decimal(item, "liqPx")?,
                venue_updated_at: item
                    .get("uTime")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|value| millis(value).ok()),
                received_at: Utc::now(),
            }));
        }
    }
    Ok(events)
}

fn reconciliation(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::ReconciliationRequired,
        code: format!("OKX_{code}"),
        message: "OKX market sequence is uncertain; a fresh snapshot is required".into(),
        retry_after_ms: None,
    }
}

fn parse_order(item: &Value) -> ConnectorResult<VenueOrder> {
    let symbol = text(item, "instId")?;
    let product = product_from_symbol(symbol);
    Ok(VenueOrder {
        instrument: instrument_id(symbol, product),
        venue_order_id: text(item, "ordId")?.into(),
        client_order_id: item
            .get("clOrdId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        side: if text(item, "side")? == "buy" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        },
        status: order_status(text(item, "state")?),
        requested_quantity: decimal(item, "sz")?,
        filled_quantity: decimal(item, "accFillSz")?,
        average_fill_price: optional_decimal(item, "avgPx")?,
        sequence: None,
        venue_updated_at: item
            .get("uTime")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| millis(value).ok()),
        received_at: Utc::now(),
    })
}

fn funding_interval(item: &Value) -> Option<u32> {
    let current = item.get("fundingTime")?.as_str()?.parse::<i64>().ok()?;
    let next = item.get("nextFundingTime")?.as_str()?.parse::<i64>().ok()?;
    u32::try_from((next - current) / 3_600_000).ok()
}

fn product_from_symbol(symbol: &str) -> ProductType {
    if symbol.ends_with("-SWAP") {
        ProductType::LinearPerpetual
    } else {
        ProductType::Spot
    }
}

fn instrument_id(symbol: &str, product: ProductType) -> InstrumentId {
    InstrumentId {
        venue: Venue::Okx,
        symbol: symbol.into(),
        product,
    }
}

const fn instrument_type(product: ProductType) -> &'static str {
    match product {
        ProductType::Spot => "SPOT",
        ProductType::LinearPerpetual => "SWAP",
    }
}

const fn side_wire(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

fn order_status(value: &str) -> OrderStatus {
    match value {
        "live" => OrderStatus::Open,
        "partially_filled" => OrderStatus::PartiallyFilled,
        "filled" => OrderStatus::Filled,
        "canceled" | "mmp_canceled" => OrderStatus::Cancelled,
        "order_failed" => OrderStatus::Rejected,
        _ => OrderStatus::Unknown,
    }
}

fn data(value: &Value) -> ConnectorResult<&Vec<Value>> {
    array(value, "data")
}

fn array<'a>(value: &'a Value, key: &str) -> ConnectorResult<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("ARRAY_MISSING"))
}

fn text<'a>(value: &'a Value, key: &str) -> ConnectorResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("TEXT_MISSING"))
}

fn decimal(value: &Value, key: &str) -> ConnectorResult<Decimal> {
    Decimal::from_str(text(value, key)?).map_err(|_| invalid("DECIMAL_INVALID"))
}

fn optional_decimal(value: &Value, key: &str) -> ConnectorResult<Option<Decimal>> {
    match value.get(key).and_then(Value::as_str) {
        None | Some("") => Ok(None),
        Some(raw) => Decimal::from_str(raw)
            .map(Some)
            .map_err(|_| invalid("DECIMAL_INVALID")),
    }
}

fn i64_text(value: &Value, key: &str) -> ConnectorResult<i64> {
    text(value, key)?
        .parse()
        .map_err(|_| invalid("INTEGER_INVALID"))
}

fn levels(value: &Value, key: &str) -> ConnectorResult<Vec<PriceLevel>> {
    array(value, key)?
        .iter()
        .map(|level| {
            let values = level
                .as_array()
                .ok_or_else(|| invalid("PRICE_LEVEL_INVALID"))?;
            Ok(PriceLevel {
                price: Decimal::from_str(
                    values.first().and_then(Value::as_str).unwrap_or_default(),
                )
                .map_err(|_| invalid("PRICE_INVALID"))?,
                quantity: Decimal::from_str(
                    values.get(1).and_then(Value::as_str).unwrap_or_default(),
                )
                .map_err(|_| invalid("QUANTITY_INVALID"))?,
            })
        })
        .collect()
}

fn value_to_string(value: &Value) -> ConnectorResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(invalid("VALUE_INVALID")),
    }
}

fn millis(value: i64) -> ConnectorResult<DateTime<Utc>> {
    Utc.timestamp_millis_opt(value)
        .single()
        .ok_or_else(|| invalid("TIMESTAMP_INVALID"))
}

fn hmac_base64(key: &[u8], message: &[u8]) -> ConnectorResult<String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| invalid("SIGNING_KEY_INVALID"))?;
    mac.update(message);
    Ok(STANDARD.encode(mac.finalize().into_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::InvalidResponse,
        code: format!("OKX_{code}"),
        message: "OKX response failed validation".into(),
        retry_after_ms: None,
    }
}

fn unsupported(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::Unsupported,
        code: format!("OKX_{code}"),
        message: "OKX operation is unavailable for this product or mode".into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BookApply, BookState, BookStatus};

    #[test]
    fn demo_header_mode_is_the_only_write_capable_environment() {
        let credentials = || ConnectorCredentials {
            api_key: "key".into(),
            api_secret: "secret".into(),
            passphrase: Some("passphrase".into()),
        };
        assert!(
            OkxConnector::new(VenueEnvironment::Demo, Some(credentials()))
                .capabilities()
                .order_submission
        );
        assert!(
            !OkxConnector::new(VenueEnvironment::Production, Some(credentials()))
                .capabilities()
                .order_submission
        );
    }

    #[test]
    fn contract_product_is_detected_from_swap_symbol() {
        assert_eq!(
            product_from_symbol("BTC-USDT-SWAP"),
            ProductType::LinearPerpetual
        );
        assert_eq!(product_from_symbol("BTC-USDT"), ProductType::Spot);
    }

    #[test]
    fn websocket_delta_preserves_previous_sequence_and_checksum() {
        let subscriptions = vec![MarketSubscription {
            instrument: instrument_id("BTC-USDT-SWAP", ProductType::LinearPerpetual),
            depth: 50,
        }];
        let frame = json!({
            "arg": {"channel": "books", "instId": "BTC-USDT-SWAP"},
            "action": "update",
            "data": [{
                "ts": "1720000000000", "prevSeqId": "100", "seqId": "102",
                "checksum": -12345,
                "bids": [["60000", "1", "0", "1"]],
                "asks": [["60001", "2", "0", "1"]]
            }]
        });
        assert!(matches!(
            parse_market_frame(&frame, &subscriptions).expect("parse"),
            Some(MarketEvent::BookDelta {
                first_sequence: 101,
                last_sequence: 102,
                checksum: Some(_),
                ..
            })
        ));
    }

    fn books_frame(action: &str, previous: &str, sequence: &str) -> Value {
        json!({
            "arg": {"channel": "books", "instId": "BTC-USDT-SWAP"},
            "action": action,
            "data": [{
                "ts": "1720000000000", "prevSeqId": previous, "seqId": sequence,
                "bids": [["60000", "1", "0", "1"]],
                "asks": [["60001", "2", "0", "1"]]
            }]
        })
    }

    fn books_subscription() -> Vec<MarketSubscription> {
        vec![MarketSubscription {
            instrument: instrument_id("BTC-USDT-SWAP", ProductType::LinearPerpetual),
            depth: 400,
        }]
    }

    /// The documented OKX sequence example: snapshot (-1, 10), update
    /// (10, 15), no-change keepalive (15, 15), maintenance reset (15, 3).
    #[test]
    fn documented_sequence_example_keeps_the_book_current_until_a_reset() {
        let subscriptions = books_subscription();
        let parse = |frame: Value| {
            parse_market_frame(&frame, &subscriptions)
                .expect("parse")
                .expect("book event")
        };
        let mut book = BookState::new(subscriptions[0].instrument.clone());
        assert_eq!(
            book.apply(&parse(books_frame("snapshot", "-1", "10"))),
            BookApply::Applied
        );
        assert_eq!(
            book.apply(&parse(books_frame("update", "10", "15"))),
            BookApply::Applied
        );
        let mut keepalive = books_frame("update", "15", "15");
        keepalive["data"][0]["bids"] = json!([]);
        keepalive["data"][0]["asks"] = json!([]);
        assert_eq!(book.apply(&parse(keepalive)), BookApply::IgnoredDuplicate);
        assert_eq!(book.status(), &BookStatus::Current);

        let reset = parse_market_frame(&books_frame("update", "15", "3"), &subscriptions)
            .expect_err("a sequence reset must fail closed");
        assert_eq!(reset.kind, ConnectorErrorKind::ReconciliationRequired);
        assert_eq!(reset.code, "OKX_SEQUENCE_RESET");
    }

    #[test]
    fn a_skipped_update_marks_the_book_stale() {
        let subscriptions = books_subscription();
        let parse = |frame: Value| {
            parse_market_frame(&frame, &subscriptions)
                .expect("parse")
                .expect("book event")
        };
        let mut book = BookState::new(subscriptions[0].instrument.clone());
        book.apply(&parse(books_frame("snapshot", "-1", "10")));
        assert_eq!(
            book.apply(&parse(books_frame("update", "12", "15"))),
            BookApply::ResetRequired
        );
        assert!(book.view().is_none());
    }

    #[test]
    fn a_resubscribe_snapshot_restores_the_book_after_a_reset() {
        let subscriptions = books_subscription();
        let mut book = BookState::new(subscriptions[0].instrument.clone());
        let snapshot = |sequence: &str| {
            parse_market_frame(&books_frame("snapshot", "-1", sequence), &subscriptions)
                .expect("parse")
                .expect("book event")
        };
        book.apply(&snapshot("500"));
        // After the reset the new range starts lower; the fresh snapshot
        // replaces the book outright rather than being compared with it.
        assert_eq!(book.apply(&snapshot("3")), BookApply::Applied);
        assert_eq!(book.status(), &BookStatus::Current);
        assert_eq!(book.view().expect("view").sequence, 3);
    }
}
