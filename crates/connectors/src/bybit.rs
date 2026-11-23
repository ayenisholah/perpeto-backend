//! Bybit V5 spot and USDT-linear adapter.

use std::{str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
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
pub struct BybitConnector {
    environment: VenueEnvironment,
    client: reqwest::Client,
    credentials: Option<Arc<ConnectorCredentials>>,
    instruments: Arc<RwLock<Vec<Instrument>>>,
}

impl BybitConnector {
    #[must_use]
    pub fn new(environment: VenueEnvironment, credentials: Option<ConnectorCredentials>) -> Self {
        Self {
            environment,
            client: reqwest::Client::new(),
            credentials: credentials.map(Arc::new),
            instruments: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn base_url(&self) -> &'static str {
        match self.environment {
            VenueEnvironment::Testnet => "https://api-testnet.bybit.com",
            VenueEnvironment::Demo => "https://api-demo.bybit.com",
            VenueEnvironment::Production => "https://api.bybit.com",
        }
    }

    async fn public_get(&self, path: &str, query: &[(&str, String)]) -> ConnectorResult<Value> {
        let query = serde_urlencoded::to_string(query).map_err(|_| invalid("QUERY_ENCODING"))?;
        self.send(Method::GET, path, &query, "", false).await
    }

    async fn private_get(&self, path: &str, query: &[(&str, String)]) -> ConnectorResult<Value> {
        let query = serde_urlencoded::to_string(query).map_err(|_| invalid("QUERY_ENCODING"))?;
        self.send(Method::GET, path, &query, "", true).await
    }

    async fn private_post(&self, path: &str, body: &Value) -> ConnectorResult<Value> {
        let body = serde_json::to_string(body).map_err(|_| invalid("BODY_ENCODING"))?;
        self.send(Method::POST, path, "", &body, true).await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &str,
        body: &str,
        authenticated: bool,
    ) -> ConnectorResult<Value> {
        let url = if query.is_empty() {
            format!("{}{path}", self.base_url())
        } else {
            format!("{}{path}?{query}", self.base_url())
        };
        let mut request = self.client.request(method.clone(), url);
        if !body.is_empty() {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_owned());
        }
        if authenticated {
            let credentials = self.credentials.as_ref().ok_or_else(|| ConnectorError {
                kind: ConnectorErrorKind::FatalConfiguration,
                code: "BYBIT_CREDENTIALS_MISSING".into(),
                message: "Bybit credentials are not configured".into(),
                retry_after_ms: None,
            })?;
            let timestamp = Utc::now().timestamp_millis().to_string();
            let receive_window = "5000";
            let payload = if method == Method::GET { query } else { body };
            let signable = format!(
                "{timestamp}{}{receive_window}{payload}",
                credentials.api_key
            );
            let signature = hmac_hex(credentials.api_secret.as_bytes(), signable.as_bytes())?;
            request = request
                .header("X-BAPI-API-KEY", &credentials.api_key)
                .header("X-BAPI-TIMESTAMP", timestamp)
                .header("X-BAPI-RECV-WINDOW", receive_window)
                .header("X-BAPI-SIGN", signature);
        }
        let response = request.send().await.map_err(|_| ConnectorError {
            kind: if method == Method::POST {
                ConnectorErrorKind::UnknownOutcome
            } else {
                ConnectorErrorKind::Transient
            },
            code: "BYBIT_NETWORK".into(),
            message: "Bybit network request failed".into(),
            retry_after_ms: None,
        })?;
        let status = response.status();
        let retry_after_ms = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| seconds.saturating_mul(1_000));
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| invalid("INVALID_JSON"))?;
        if status.as_u16() == 429 {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::RateLimited,
                code: "BYBIT_RATE_LIMITED".into(),
                message: "Bybit rate limit reached".into(),
                retry_after_ms,
            });
        }
        if status.is_server_error() {
            return Err(ConnectorError {
                kind: if method == Method::POST {
                    ConnectorErrorKind::UnknownOutcome
                } else {
                    ConnectorErrorKind::Transient
                },
                code: format!("BYBIT_HTTP_{}", status.as_u16()),
                message: "Bybit server error".into(),
                retry_after_ms,
            });
        }
        let code = value.get("retCode").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return Err(ConnectorError {
                kind: if code == 10006 {
                    ConnectorErrorKind::RateLimited
                } else {
                    ConnectorErrorKind::InvalidResponse
                },
                code: format!("BYBIT_{code}"),
                message: value
                    .get("retMsg")
                    .and_then(Value::as_str)
                    .unwrap_or("Bybit request failed")
                    .to_owned(),
                retry_after_ms,
            });
        }
        Ok(value)
    }

    async fn instrument_pages(&self, product: ProductType) -> ConnectorResult<Vec<Instrument>> {
        let category = category(product);
        let mut cursor = String::new();
        let mut instruments = Vec::new();
        for _ in 0..20 {
            let mut query = vec![
                ("category", category.to_owned()),
                ("limit", "1000".to_owned()),
            ];
            if !cursor.is_empty() {
                query.push(("cursor", cursor.clone()));
            }
            let response = self
                .public_get("/v5/market/instruments-info", &query)
                .await?;
            let result = &response["result"];
            for item in array(result, "list")? {
                let symbol = text(item, "symbol")?;
                if !matches!(symbol, "BTCUSDT" | "ETHUSDT") {
                    continue;
                }
                let lot = &item["lotSizeFilter"];
                let price_filter = &item["priceFilter"];
                instruments.push(Instrument {
                    id: InstrumentId {
                        venue: Venue::Bybit,
                        symbol: symbol.into(),
                        product,
                    },
                    canonical_underlying: symbol.trim_end_matches("USDT").into(),
                    quote_asset: "USDT".into(),
                    settlement_asset: "USDT".into(),
                    margin_asset: "USDT".into(),
                    contract_multiplier: Decimal::ONE,
                    price_tick: decimal(price_filter, "tickSize")?,
                    quantity_step: decimal(lot, "qtyStep")?,
                    minimum_quantity: decimal(lot, "minOrderQty")?,
                    maximum_quantity: optional_decimal(lot, "maxOrderQty")?,
                    minimum_notional: optional_decimal(lot, "minNotionalValue")?
                        .unwrap_or(Decimal::ZERO),
                    funding_interval_hours: item
                        .get("fundingInterval")
                        .and_then(|minutes| {
                            minutes
                                .as_u64()
                                .and_then(|value| u32::try_from(value).ok())
                                .or_else(|| {
                                    minutes.as_str().and_then(|value| value.parse::<u32>().ok())
                                })
                        })
                        .map(|minutes| minutes / 60),
                    funding_cap: optional_decimal(item, "upperFundingRate")?,
                    funding_floor: optional_decimal(item, "lowerFundingRate")?,
                    maximum_leverage: optional_decimal(&item["leverageFilter"], "maxLeverage")?,
                    trading: text(item, "status")? == "Trading",
                    metadata_version: format!("bybit-v5-{}", Utc::now().date_naive()),
                    venue_updated_at: None,
                    received_at: Utc::now(),
                });
            }
            cursor = result
                .get("nextPageCursor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if cursor.is_empty() {
                return Ok(instruments);
            }
        }
        Err(invalid("INSTRUMENT_PAGINATION_LIMIT"))
    }

    // Implemented but not yet wired into the WebSocket stream. Binance calls its
    // equivalent on a transport reconnect to rebuild the book and re-anchor the
    // sequence; Bybit's reconnect path does not, so it currently recovers only
    // through its own sequence-reset handling. Wiring this needs credentialed
    // venue evidence and belongs to the open M3 certification work.
    #[expect(dead_code, reason = "pending M3 reconnect snapshot bridge")]
    async fn book_snapshot(
        &self,
        subscription: &MarketSubscription,
    ) -> ConnectorResult<MarketEvent> {
        let response = self
            .public_get(
                "/v5/market/orderbook",
                &[
                    ("category", category(subscription.instrument.product).into()),
                    ("symbol", subscription.instrument.symbol.clone()),
                    ("limit", subscription.depth.clamp(1, 200).to_string()),
                ],
            )
            .await?;
        let result = &response["result"];
        let received_at = Utc::now();
        Ok(MarketEvent::BookSnapshot {
            instrument: subscription.instrument.clone(),
            sequence: u64_value(result, "seq").or_else(|_| u64_value(result, "u"))?,
            bids: levels(result, "b")?,
            asks: levels(result, "a")?,
            venue_time: millis(
                response
                    .get("time")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| received_at.timestamp_millis()),
            )?,
            received_at,
            checksum: None,
        })
    }

    fn instrument_for(&self, symbol: &str, product: ProductType) -> InstrumentId {
        InstrumentId {
            venue: Venue::Bybit,
            symbol: symbol.into(),
            product,
        }
    }

    async fn orders_for(
        &self,
        _account: &AccountId,
        history: bool,
    ) -> ConnectorResult<Vec<VenueOrder>> {
        let mut orders = Vec::new();
        for product in [ProductType::Spot, ProductType::LinearPerpetual] {
            let path = if history {
                "/v5/order/history"
            } else {
                "/v5/order/realtime"
            };
            let response = self
                .private_get(path, &[("category", category(product).into())])
                .await?;
            for item in array(&response["result"], "list")? {
                orders.push(parse_order(item, product)?);
            }
        }
        Ok(orders)
    }
}

#[async_trait]
impl VenueConnector for BybitConnector {
    fn venue(&self) -> Venue {
        Venue::Bybit
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            products: vec![ProductType::Spot, ProductType::LinearPerpetual],
            market_stream: true,
            private_stream: self.credentials.is_some(),
            funding_history: true,
            order_submission: self.credentials.is_some() && self.environment.is_non_production(),
            borrow: false,
            internal_transfer: false,
        }
    }

    async fn health(&self) -> ConnectorResult<VenueHealth> {
        let timestamp = self.server_time().await?;
        let observed_at = Utc::now();
        let drift = (observed_at - timestamp.venue_time).num_milliseconds();
        Ok(VenueHealth {
            venue: Venue::Bybit,
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
        let response = self.public_get("/v5/market/time", &[]).await?;
        let venue_time = response["result"]
            .get("timeNano")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .map(|nanos| nanos / 1_000_000)
            .or_else(|| response.get("time").and_then(Value::as_i64))
            .ok_or_else(|| invalid("SERVER_TIME_MISSING"))?;
        Ok(VenueTimestamp {
            venue_time: millis(venue_time)?,
            received_at: Utc::now(),
        })
    }

    async fn instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        let mut instruments = self.instrument_pages(ProductType::Spot).await?;
        instruments.extend(self.instrument_pages(ProductType::LinearPerpetual).await?);
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
        let product = subscriptions[0].instrument.product;
        if subscriptions
            .iter()
            .any(|subscription| subscription.instrument.product != product)
        {
            return Err(unsupported("MIXED_PRODUCT_STREAM"));
        }
        let args = subscriptions
            .iter()
            .map(|subscription| {
                format!(
                    "orderbook.{}.{}",
                    subscription.depth, subscription.instrument.symbol
                )
            })
            .collect::<Vec<_>>();
        let stream_subscriptions = Arc::new(subscriptions);
        let frames = connect_json(JsonWsConfig {
            name: "BYBIT",
            url: bybit_public_ws_url(self.environment, product).to_owned(),
            initial_messages: vec![json!({"op": "subscribe", "args": args})],
            heartbeat: Some(r#"{"op":"ping"}"#.into()),
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
            code: "BYBIT_CREDENTIALS_MISSING".into(),
            message: "Bybit credentials are not configured".into(),
            retry_after_ms: None,
        })?;
        let expires = Utc::now().timestamp_millis().saturating_add(10_000);
        let signature = hmac_hex(
            credentials.api_secret.as_bytes(),
            format!("GET/realtime{expires}").as_bytes(),
        )?;
        let frames = connect_json(JsonWsConfig {
            name: "BYBIT_PRIVATE",
            url: bybit_private_ws_url(self.environment).to_owned(),
            initial_messages: vec![
                json!({"op": "auth", "args": [credentials.api_key.clone(), expires, signature]}),
                json!({"op": "subscribe", "args": ["order", "execution", "wallet", "position"]}),
            ],
            heartbeat: Some(r#"{"op":"ping"}"#.into()),
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
                    "/v5/account/fee-rate",
                    &[("category", category(product).into())],
                )
                .await?;
            let Some(item) = array(&response["result"], "list")?.first() else {
                continue;
            };
            fees.push(FeeSchedule {
                product,
                maker: decimal(item, "makerFeeRate")?,
                taker: decimal(item, "takerFeeRate")?,
                effective_at: Utc::now(),
            });
        }
        Ok(fees)
    }

    async fn current_funding(&self, instrument: &InstrumentId) -> ConnectorResult<FundingQuote> {
        if instrument.product != ProductType::LinearPerpetual {
            return Err(unsupported("FUNDING_SPOT_UNSUPPORTED"));
        }
        let response = self
            .public_get(
                "/v5/market/tickers",
                &[
                    ("category", "linear".into()),
                    ("symbol", instrument.symbol.clone()),
                ],
            )
            .await?;
        let item = array(&response["result"], "list")?
            .first()
            .ok_or_else(|| invalid("FUNDING_EMPTY"))?;
        let interval = self
            .instruments
            .read()
            .await
            .iter()
            .find(|value| value.id == *instrument)
            .and_then(|value| value.funding_interval_hours)
            .unwrap_or(8);
        Ok(FundingQuote {
            instrument: instrument.clone(),
            rate: decimal(item, "fundingRate")?,
            interval_hours: interval,
            mark_price: decimal(item, "markPrice")?,
            index_price: decimal(item, "indexPrice")?,
            next_funding_at: millis(i64_text(item, "nextFundingTime")?)?,
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
                "/v5/market/funding/history",
                &[
                    ("category", "linear".into()),
                    ("symbol", instrument.symbol.clone()),
                    ("startTime", range.start.timestamp_millis().to_string()),
                    ("endTime", range.end.timestamp_millis().to_string()),
                    ("limit", "200".into()),
                ],
            )
            .await?;
        let mut quotes = Vec::new();
        for item in array(&response["result"], "list")? {
            quotes.push(FundingQuote {
                instrument: instrument.clone(),
                rate: decimal(item, "fundingRate")?,
                interval_hours: 8,
                mark_price: Decimal::ZERO,
                index_price: Decimal::ZERO,
                next_funding_at: millis(i64_text(item, "fundingRateTimestamp")?)?,
                observed_at: Utc::now(),
            });
        }
        Ok(quotes)
    }

    async fn balances(&self, _account: &AccountId) -> ConnectorResult<Vec<Balance>> {
        let response = self
            .private_get(
                "/v5/account/wallet-balance",
                &[("accountType", "UNIFIED".into())],
            )
            .await?;
        let mut balances = Vec::new();
        for wallet in array(&response["result"], "list")? {
            for item in array(wallet, "coin")? {
                let total = decimal(item, "walletBalance")?;
                let available = optional_decimal(item, "availableToWithdraw")?.unwrap_or(total);
                balances.push(Balance {
                    asset: text(item, "coin")?.into(),
                    total,
                    available,
                    locked: total - available,
                    venue_updated_at: None,
                    received_at: Utc::now(),
                });
            }
        }
        Ok(balances)
    }

    async fn positions(&self, _account: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
        let response = self
            .private_get(
                "/v5/position/list",
                &[
                    ("category", "linear".into()),
                    ("settleCoin", "USDT".into()),
                    ("limit", "200".into()),
                ],
            )
            .await?;
        let mut positions = Vec::new();
        for item in array(&response["result"], "list")? {
            let mut quantity = decimal(item, "size")?;
            if text(item, "side")? == "Sell" {
                quantity = -quantity;
            }
            if quantity.is_zero() {
                continue;
            }
            positions.push(VenuePosition {
                instrument: self
                    .instrument_for(text(item, "symbol")?, ProductType::LinearPerpetual),
                quantity,
                entry_price: decimal(item, "avgPrice")?,
                mark_price: decimal(item, "markPrice")?,
                leverage: decimal(item, "leverage")?,
                liquidation_price: optional_decimal(item, "liqPrice")?,
                venue_updated_at: item
                    .get("updatedTime")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|value| millis(value).ok()),
                received_at: Utc::now(),
            });
        }
        Ok(positions)
    }

    async fn open_orders(&self, account: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
        self.orders_for(account, false).await
    }

    async fn fills(&self, _account: &AccountId, cursor: Option<&str>) -> ConnectorResult<FillPage> {
        let response = self
            .private_get(
                "/v5/execution/list",
                &[
                    ("category", "linear".into()),
                    ("limit", "100".into()),
                    ("cursor", cursor.unwrap_or_default().into()),
                ],
            )
            .await?;
        let mut fills = Vec::new();
        for item in array(&response["result"], "list")? {
            fills.push(Fill {
                venue_fill_id: text(item, "execId")?.into(),
                venue_order_id: text(item, "orderId")?.into(),
                instrument: self
                    .instrument_for(text(item, "symbol")?, ProductType::LinearPerpetual),
                quantity: decimal(item, "execQty")?,
                price: decimal(item, "execPrice")?,
                fee: decimal(item, "execFee")?,
                fee_asset: item
                    .get("feeCurrency")
                    .and_then(Value::as_str)
                    .unwrap_or("USDT")
                    .into(),
                occurred_at: millis(i64_text(item, "execTime")?)?,
                received_at: Utc::now(),
            });
        }
        Ok(FillPage {
            fills,
            next_cursor: response["result"]
                .get("nextPageCursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        })
    }

    async fn funding_ledger(
        &self,
        _account: &AccountId,
        range: &TimeRange,
    ) -> ConnectorResult<Vec<FundingLedgerEntry>> {
        let response = self
            .private_get(
                "/v5/account/transaction-log",
                &[
                    ("accountType", "UNIFIED".into()),
                    ("category", "linear".into()),
                    ("type", "SETTLEMENT".into()),
                    ("startTime", range.start.timestamp_millis().to_string()),
                    ("endTime", range.end.timestamp_millis().to_string()),
                ],
            )
            .await?;
        let mut entries = Vec::new();
        for item in array(&response["result"], "list")? {
            let symbol = text(item, "symbol")?;
            entries.push(FundingLedgerEntry {
                venue_entry_id: text(item, "transactionTime")?.to_owned() + symbol,
                instrument: self.instrument_for(symbol, ProductType::LinearPerpetual),
                amount: optional_decimal(item, "funding")?
                    .or(optional_decimal(item, "cashFlow")?)
                    .unwrap_or(Decimal::ZERO),
                asset: text(item, "currency")?.into(),
                occurred_at: millis(i64_text(item, "transactionTime")?)?,
            });
        }
        Ok(entries)
    }

    async fn prepare_order(&self, order: &CanonicalOrderIntent) -> ConnectorResult<PreparedOrder> {
        if order.instrument.venue != Venue::Bybit {
            return Err(invalid("VENUE_MISMATCH"));
        }
        let payload = json!({
            "category": category(order.instrument.product),
            "symbol": order.instrument.symbol,
            "side": side_wire(order.side),
            "orderType": "Limit",
            "qty": order.base_quantity.to_string(),
            "price": order.limit_price.to_string(),
            "timeInForce": order.time_in_force,
            "orderLinkId": order.client_order_id,
            "reduceOnly": order.reduce_only,
            "positionIdx": 0,
            "isLeverage": 0,
        });
        let encoded = serde_json::to_vec(&payload).map_err(|_| invalid("BODY_ENCODING"))?;
        Ok(PreparedOrder {
            intent: order.clone(),
            native_symbol: order.instrument.symbol.clone(),
            native_quantity: order.base_quantity,
            equivalent_base_quantity: order.base_quantity,
            native_price: order.limit_price,
            projected_delta_usd: order.base_quantity * order.limit_price,
            payload,
            payload_hash: hex(&Sha256::digest(encoded)),
        })
    }

    async fn submit_order(&self, order: &PreparedOrder) -> ConnectorResult<OrderAck> {
        if !self.environment.is_non_production() {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::FatalConfiguration,
                code: "BYBIT_PRODUCTION_WRITE_FORBIDDEN".into(),
                message: "production writes are disabled".into(),
                retry_after_ms: None,
            });
        }
        let response = self
            .private_post("/v5/order/create", &order.payload)
            .await?;
        Ok(OrderAck {
            venue_order_id: response["result"]
                .get("orderId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            client_order_id: order.intent.client_order_id.clone(),
            status: OrderStatus::Pending,
            acknowledged_at: Utc::now(),
        })
    }

    async fn cancel_order(&self, request: &CancelRequest) -> ConnectorResult<CancelAck> {
        if !self.environment.is_non_production() {
            return Err(unsupported("PRODUCTION_WRITE_FORBIDDEN"));
        }
        self.private_post(
            "/v5/order/cancel",
            &json!({
                "category": category(request.instrument.product),
                "symbol": request.instrument.symbol,
                "orderId": request.venue_order_id,
                "orderLinkId": request.client_order_id,
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
        account: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>> {
        let orders = self.orders_for(account, true).await?;
        Ok(orders.into_iter().find(|order| match key {
            OrderLookup::VenueOrderId(id) => order.venue_order_id == *id,
            OrderLookup::ClientOrderId(id) => order.client_order_id.as_deref() == Some(id),
        }))
    }
}

fn bybit_public_ws_url(environment: VenueEnvironment, product: ProductType) -> &'static str {
    match (environment, product) {
        (VenueEnvironment::Testnet, ProductType::Spot) => {
            "wss://stream-testnet.bybit.com/v5/public/spot"
        }
        (VenueEnvironment::Testnet, ProductType::LinearPerpetual) => {
            "wss://stream-testnet.bybit.com/v5/public/linear"
        }
        (_, ProductType::Spot) => "wss://stream.bybit.com/v5/public/spot",
        (_, ProductType::LinearPerpetual) => "wss://stream.bybit.com/v5/public/linear",
    }
}

fn bybit_private_ws_url(environment: VenueEnvironment) -> &'static str {
    match environment {
        VenueEnvironment::Testnet => "wss://stream-testnet.bybit.com/v5/private",
        VenueEnvironment::Demo => "wss://stream-demo.bybit.com/v5/private",
        VenueEnvironment::Production => "wss://stream.bybit.com/v5/private",
    }
}

fn parse_market_frame(
    frame: &Value,
    subscriptions: &[MarketSubscription],
) -> ConnectorResult<Option<MarketEvent>> {
    let Some(topic) = frame.get("topic").and_then(Value::as_str) else {
        if frame.get("success").is_some()
            || frame.get("op").is_some()
            || frame.get("_perpeto_transport").is_some()
        {
            return Ok(None);
        }
        return Err(invalid("WS_TOPIC_MISSING"));
    };
    let symbol = topic
        .rsplit('.')
        .next()
        .ok_or_else(|| invalid("WS_TOPIC_INVALID"))?;
    let subscription = subscriptions
        .iter()
        .find(|subscription| subscription.instrument.symbol == symbol)
        .ok_or_else(|| invalid("WS_UNSUBSCRIBED_SYMBOL"))?;
    let data = frame
        .get("data")
        .ok_or_else(|| invalid("WS_DATA_MISSING"))?;
    let sequence = u64_value(data, "u")?;
    let timestamp = frame
        .get("cts")
        .or_else(|| frame.get("ts"))
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("WS_TIMESTAMP_MISSING"))?;
    let common = (
        subscription.instrument.clone(),
        levels(data, "b")?,
        levels(data, "a")?,
        millis(timestamp)?,
        Utc::now(),
    );
    match frame.get("type").and_then(Value::as_str) {
        Some("snapshot") => Ok(Some(MarketEvent::BookSnapshot {
            instrument: common.0,
            sequence,
            bids: common.1,
            asks: common.2,
            venue_time: common.3,
            received_at: common.4,
            checksum: None,
        })),
        Some("delta") if sequence == 1 => Ok(Some(MarketEvent::BookSnapshot {
            instrument: common.0,
            sequence,
            bids: common.1,
            asks: common.2,
            venue_time: common.3,
            received_at: common.4,
            checksum: None,
        })),
        Some("delta") => Ok(Some(MarketEvent::BookDelta {
            instrument: common.0,
            first_sequence: sequence,
            last_sequence: sequence,
            bids: common.1,
            asks: common.2,
            venue_time: common.3,
            received_at: common.4,
            checksum: None,
        })),
        _ => Err(invalid("WS_TYPE_INVALID")),
    }
}

fn parse_private_frame(frame: &Value) -> ConnectorResult<Vec<PrivateEvent>> {
    if frame.get("_perpeto_transport").is_some() {
        return Ok(vec![PrivateEvent::Heartbeat(Utc::now())]);
    }
    let Some(topic) = frame.get("topic").and_then(Value::as_str) else {
        if frame.get("op").is_some() || frame.get("success").is_some() {
            return Ok(Vec::new());
        }
        return Err(invalid("PRIVATE_TOPIC_MISSING"));
    };
    let items = array(frame, "data")?;
    let mut events = Vec::new();
    for item in items {
        if topic.starts_with("order") {
            events.push(PrivateEvent::Order(parse_order(
                item,
                product_from_category(item),
            )?));
        } else if topic.starts_with("execution") {
            events.push(PrivateEvent::Fill(Fill {
                venue_fill_id: text(item, "execId")?.into(),
                venue_order_id: text(item, "orderId")?.into(),
                instrument: InstrumentId {
                    venue: Venue::Bybit,
                    symbol: text(item, "symbol")?.into(),
                    product: product_from_category(item),
                },
                quantity: decimal(item, "execQty")?,
                price: decimal(item, "execPrice")?,
                fee: decimal(item, "execFee")?,
                fee_asset: item
                    .get("feeCurrency")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN")
                    .into(),
                occurred_at: millis(i64_text(item, "execTime")?)?,
                received_at: Utc::now(),
            }));
        } else if topic.starts_with("position") {
            let mut quantity = decimal(item, "size")?;
            if item.get("side").and_then(Value::as_str) == Some("Sell") {
                quantity = -quantity;
            }
            events.push(PrivateEvent::Position(VenuePosition {
                instrument: InstrumentId {
                    venue: Venue::Bybit,
                    symbol: text(item, "symbol")?.into(),
                    product: product_from_category(item),
                },
                quantity,
                entry_price: optional_decimal(item, "entryPrice")?.unwrap_or(Decimal::ZERO),
                mark_price: optional_decimal(item, "markPrice")?.unwrap_or(Decimal::ZERO),
                leverage: optional_decimal(item, "leverage")?.unwrap_or(Decimal::ONE),
                liquidation_price: optional_decimal(item, "liqPrice")?,
                venue_updated_at: item
                    .get("updatedTime")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|value| millis(value).ok()),
                received_at: Utc::now(),
            }));
        } else if topic.starts_with("wallet") {
            for coin in item
                .get("coin")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let total = decimal(coin, "walletBalance")?;
                let available = optional_decimal(coin, "availableToWithdraw")?
                    .or(optional_decimal(coin, "availableBalance")?)
                    .unwrap_or(total);
                events.push(PrivateEvent::Balance(Balance {
                    asset: text(coin, "coin")?.into(),
                    total,
                    available,
                    locked: (total - available).max(Decimal::ZERO),
                    venue_updated_at: None,
                    received_at: Utc::now(),
                }));
            }
        }
    }
    Ok(events)
}

fn product_from_category(item: &Value) -> ProductType {
    if item.get("category").and_then(Value::as_str) == Some("spot") {
        ProductType::Spot
    } else {
        ProductType::LinearPerpetual
    }
}

fn parse_order(item: &Value, product: ProductType) -> ConnectorResult<VenueOrder> {
    Ok(VenueOrder {
        instrument: InstrumentId {
            venue: Venue::Bybit,
            symbol: text(item, "symbol")?.into(),
            product,
        },
        venue_order_id: text(item, "orderId")?.into(),
        client_order_id: item
            .get("orderLinkId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        side: if text(item, "side")? == "Buy" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        },
        status: order_status(text(item, "orderStatus")?),
        requested_quantity: decimal(item, "qty")?,
        filled_quantity: decimal(item, "cumExecQty")?,
        average_fill_price: optional_decimal(item, "avgPrice")?,
        sequence: None,
        venue_updated_at: item
            .get("updatedTime")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| millis(value).ok()),
        received_at: Utc::now(),
    })
}

const fn category(product: ProductType) -> &'static str {
    match product {
        ProductType::Spot => "spot",
        ProductType::LinearPerpetual => "linear",
    }
}

const fn side_wire(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "Buy",
        OrderSide::Sell => "Sell",
    }
}

fn order_status(value: &str) -> OrderStatus {
    match value {
        "New" | "Untriggered" | "Active" => OrderStatus::Open,
        "PartiallyFilled" => OrderStatus::PartiallyFilled,
        "Filled" => OrderStatus::Filled,
        "Cancelled" | "Deactivated" => OrderStatus::Cancelled,
        "Rejected" => OrderStatus::Rejected,
        _ => OrderStatus::Unknown,
    }
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

fn u64_value(value: &Value, key: &str) -> ConnectorResult<u64> {
    value
        .get(key)
        .and_then(|raw| raw.as_u64().or_else(|| raw.as_str()?.parse().ok()))
        .ok_or_else(|| invalid("SEQUENCE_INVALID"))
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

fn millis(value: i64) -> ConnectorResult<DateTime<Utc>> {
    Utc.timestamp_millis_opt(value)
        .single()
        .ok_or_else(|| invalid("TIMESTAMP_INVALID"))
}

fn hmac_hex(key: &[u8], message: &[u8]) -> ConnectorResult<String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| invalid("SIGNING_KEY_INVALID"))?;
    mac.update(message);
    Ok(hex(&mac.finalize().into_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::InvalidResponse,
        code: format!("BYBIT_{code}"),
        message: "Bybit response failed validation".into(),
        retry_after_ms: None,
    }
}

fn unsupported(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::Unsupported,
        code: format!("BYBIT_{code}"),
        message: "Bybit operation is unavailable for this product or mode".into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BookApply, BookState, BookStatus};

    #[test]
    fn maps_terminal_and_partial_order_states() {
        assert_eq!(
            order_status("PartiallyFilled"),
            OrderStatus::PartiallyFilled
        );
        assert_eq!(order_status("Filled"), OrderStatus::Filled);
        assert_eq!(order_status("Cancelled"), OrderStatus::Cancelled);
    }

    #[test]
    fn production_never_advertises_submission() {
        let connector = BybitConnector::new(
            VenueEnvironment::Production,
            Some(ConnectorCredentials {
                api_key: "key".into(),
                api_secret: "secret".into(),
                passphrase: None,
            }),
        );
        assert!(!connector.capabilities().order_submission);
    }

    #[test]
    fn websocket_snapshot_and_restart_reset_are_normalized() {
        let subscriptions = vec![MarketSubscription {
            instrument: InstrumentId {
                venue: Venue::Bybit,
                symbol: "BTCUSDT".into(),
                product: ProductType::LinearPerpetual,
            },
            depth: 50,
        }];
        let snapshot = json!({
            "topic": "orderbook.50.BTCUSDT",
            "type": "snapshot",
            "cts": 1_720_000_000_000_i64,
            "data": {"u": 100, "seq": 200, "b": [["60000", "1"]], "a": [["60001", "2"]]}
        });
        assert!(matches!(
            parse_market_frame(&snapshot, &subscriptions).expect("parse"),
            Some(MarketEvent::BookSnapshot { sequence: 100, .. })
        ));
        let restart = json!({
            "topic": "orderbook.50.BTCUSDT",
            "type": "delta",
            "ts": 1_720_000_000_001_i64,
            "data": {"u": 1, "seq": 201, "b": [], "a": []}
        });
        assert!(matches!(
            parse_market_frame(&restart, &subscriptions).expect("parse"),
            Some(MarketEvent::BookSnapshot { sequence: 1, .. })
        ));
    }

    /// Bybit documents that a successful subscription is answered with a
    /// `snapshot` and that any `snapshot` resets the local book. A reconnect
    /// resubscribes, so the transport marker carries no book data and the
    /// next snapshot replaces the book even when its update ID is lower.
    #[test]
    fn a_reconnect_is_recovered_by_the_resubscribe_snapshot() {
        let subscriptions = vec![MarketSubscription {
            instrument: InstrumentId {
                venue: Venue::Bybit,
                symbol: "BTCUSDT".into(),
                product: ProductType::LinearPerpetual,
            },
            depth: 50,
        }];
        let frame = |kind: &str, update: u64| {
            json!({
                "topic": "orderbook.50.BTCUSDT",
                "type": kind,
                "cts": 1_720_000_000_000_i64,
                "data": {"u": update, "seq": 900, "b": [["60000", "1"]], "a": [["60001", "2"]]}
            })
        };
        let parse = |value: Value| {
            parse_market_frame(&value, &subscriptions)
                .expect("parse")
                .expect("book event")
        };
        let mut book = BookState::new(subscriptions[0].instrument.clone());
        assert_eq!(
            book.apply(&parse(frame("snapshot", 100))),
            BookApply::Applied
        );
        assert_eq!(book.apply(&parse(frame("delta", 101))), BookApply::Applied);
        assert!(
            parse_market_frame(&json!({"_perpeto_transport": "connected"}), &subscriptions)
                .expect("parse")
                .is_none()
        );
        assert_eq!(
            book.apply(&parse(frame("snapshot", 40))),
            BookApply::Applied
        );
        assert_eq!(book.status(), &BookStatus::Current);
        assert_eq!(book.view().expect("view").sequence, 40);
    }
}
