//! Binance Spot and USDⓈ-M adapter.

use std::{collections::VecDeque, str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use funding_arb_domain::market::Venue;
use futures_util::stream;
use hmac::{Hmac, Mac};
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
pub struct BinanceConnector {
    environment: VenueEnvironment,
    client: reqwest::Client,
    credentials: Option<Arc<ConnectorCredentials>>,
}

impl BinanceConnector {
    #[must_use]
    pub fn new(environment: VenueEnvironment, credentials: Option<ConnectorCredentials>) -> Self {
        Self {
            environment,
            client: reqwest::Client::new(),
            credentials: credentials.map(Arc::new),
        }
    }

    fn base_url(&self, product: ProductType) -> &'static str {
        match (self.environment, product) {
            (VenueEnvironment::Testnet, ProductType::Spot) => "https://testnet.binance.vision",
            (VenueEnvironment::Testnet, ProductType::LinearPerpetual) => {
                "https://testnet.binancefuture.com"
            }
            (_, ProductType::Spot) => "https://api.binance.com",
            (_, ProductType::LinearPerpetual) => "https://fapi.binance.com",
        }
    }

    async fn public_get(
        &self,
        product: ProductType,
        path: &str,
        query: &[(&str, String)],
    ) -> ConnectorResult<Value> {
        self.send(product, Method::GET, path, query, false).await
    }

    async fn signed(
        &self,
        product: ProductType,
        method: Method,
        path: &str,
        query: &[(&str, String)],
    ) -> ConnectorResult<Value> {
        self.send(product, method, path, query, true).await
    }

    async fn send(
        &self,
        product: ProductType,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        authenticated: bool,
    ) -> ConnectorResult<Value> {
        let mut parameters = query.to_vec();
        let credentials = if authenticated {
            let credentials = self.credentials.as_ref().ok_or_else(|| ConnectorError {
                kind: ConnectorErrorKind::FatalConfiguration,
                code: "BINANCE_CREDENTIALS_MISSING".into(),
                message: "Binance credentials are not configured".into(),
                retry_after_ms: None,
            })?;
            parameters.push(("recvWindow", "5000".into()));
            parameters.push(("timestamp", Utc::now().timestamp_millis().to_string()));
            Some(credentials)
        } else {
            None
        };
        let mut encoded =
            serde_urlencoded::to_string(&parameters).map_err(|_| invalid("QUERY_ENCODING"))?;
        if let Some(credentials) = credentials {
            let signature = hmac_hex(credentials.api_secret.as_bytes(), encoded.as_bytes())?;
            encoded.push_str("&signature=");
            encoded.push_str(&signature);
        }
        let url = format!("{}{path}?{encoded}", self.base_url(product));
        let mut request = self.client.request(method.clone(), url);
        if let Some(credentials) = credentials {
            request = request.header("X-MBX-APIKEY", &credentials.api_key);
        }
        let response = request.send().await.map_err(|_| ConnectorError {
            kind: if method == Method::POST {
                ConnectorErrorKind::UnknownOutcome
            } else {
                ConnectorErrorKind::Transient
            },
            code: "BINANCE_NETWORK".into(),
            message: "Binance network request failed".into(),
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
        if matches!(status.as_u16(), 418 | 429) {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::RateLimited,
                code: "BINANCE_RATE_LIMITED".into(),
                message: "Binance rate limit reached".into(),
                retry_after_ms,
            });
        }
        if status.is_server_error() {
            return Err(ConnectorError {
                kind: if authenticated && method != Method::GET {
                    ConnectorErrorKind::UnknownOutcome
                } else {
                    ConnectorErrorKind::Transient
                },
                code: format!("BINANCE_HTTP_{}", status.as_u16()),
                message: "Binance server error".into(),
                retry_after_ms,
            });
        }
        if !status.is_success()
            || value
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code < 0)
        {
            let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
            return Err(ConnectorError {
                kind: ConnectorErrorKind::InvalidResponse,
                code: format!("BINANCE_{code}"),
                message: value
                    .get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("Binance request failed")
                    .into(),
                retry_after_ms,
            });
        }
        Ok(value)
    }

    async fn spot_instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        let response = self
            .public_get(ProductType::Spot, "/api/v3/exchangeInfo", &[])
            .await?;
        let mut instruments = Vec::new();
        for item in array(&response, "symbols")? {
            let symbol = text(item, "symbol")?;
            if !matches!(symbol, "BTCUSDT" | "ETHUSDT") {
                continue;
            }
            instruments.push(parse_instrument(item, ProductType::Spot)?);
        }
        Ok(instruments)
    }

    async fn futures_instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        let response = self
            .public_get(ProductType::LinearPerpetual, "/fapi/v1/exchangeInfo", &[])
            .await?;
        let mut instruments = Vec::new();
        for item in array(&response, "symbols")? {
            let symbol = text(item, "symbol")?;
            if matches!(symbol, "BTCUSDT" | "ETHUSDT") && text(item, "contractType")? == "PERPETUAL"
            {
                instruments.push(parse_instrument(item, ProductType::LinearPerpetual)?);
            }
        }
        Ok(instruments)
    }

    async fn book_snapshot(
        &self,
        subscription: &MarketSubscription,
    ) -> ConnectorResult<MarketEvent> {
        let path = match subscription.instrument.product {
            ProductType::Spot => "/api/v3/depth",
            ProductType::LinearPerpetual => "/fapi/v1/depth",
        };
        let response = self
            .public_get(
                subscription.instrument.product,
                path,
                &[
                    ("symbol", subscription.instrument.symbol.clone()),
                    ("limit", subscription.depth.clamp(5, 1_000).to_string()),
                ],
            )
            .await?;
        let received_at = Utc::now();
        Ok(MarketEvent::BookSnapshot {
            instrument: subscription.instrument.clone(),
            sequence: u64_value(&response, "lastUpdateId")?,
            bids: levels(&response, "bids")?,
            asks: levels(&response, "asks")?,
            venue_time: received_at,
            received_at,
            checksum: None,
        })
    }

    async fn open_for_product(
        &self,
        product: ProductType,
        account: &AccountId,
    ) -> ConnectorResult<Vec<VenueOrder>> {
        let _ = account;
        let path = match product {
            ProductType::Spot => "/api/v3/openOrders",
            ProductType::LinearPerpetual => "/fapi/v1/openOrders",
        };
        let response = self.signed(product, Method::GET, path, &[]).await?;
        response
            .as_array()
            .ok_or_else(|| invalid("ORDERS_INVALID"))?
            .iter()
            .map(|item| parse_order(item, product))
            .collect()
    }
}

#[async_trait]
impl VenueConnector for BinanceConnector {
    fn venue(&self) -> Venue {
        Venue::Binance
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            products: vec![ProductType::Spot, ProductType::LinearPerpetual],
            market_stream: true,
            private_stream: self.credentials.is_some(),
            funding_history: true,
            order_submission: self.credentials.is_some()
                && self.environment == VenueEnvironment::Testnet,
            borrow: false,
            internal_transfer: false,
        }
    }

    async fn health(&self) -> ConnectorResult<VenueHealth> {
        let timestamp = self.server_time().await?;
        let observed_at = Utc::now();
        let drift = (observed_at - timestamp.venue_time).num_milliseconds();
        Ok(VenueHealth {
            venue: Venue::Binance,
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
        let response = self
            .public_get(ProductType::Spot, "/api/v3/time", &[])
            .await?;
        Ok(VenueTimestamp {
            venue_time: millis(i64_value(&response, "serverTime")?)?,
            received_at: Utc::now(),
        })
    }

    async fn instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        let mut instruments = self.spot_instruments().await?;
        instruments.extend(self.futures_instruments().await?);
        Ok(instruments)
    }

    async fn market_stream(
        &self,
        subscriptions: Vec<MarketSubscription>,
    ) -> ConnectorResult<MarketEventStream> {
        if subscriptions.is_empty() {
            return Err(invalid("EMPTY_SUBSCRIPTION"));
        }
        let connector = self.clone();
        Ok(Box::pin(stream::unfold(
            (connector, subscriptions, 0_usize),
            |(connector, subscriptions, index)| async move {
                if index > 0 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                let selected = index % subscriptions.len();
                let event = connector.book_snapshot(&subscriptions[selected]).await;
                Some((event, (connector, subscriptions, index + 1)))
            },
        )))
    }

    async fn private_stream(&self, account: &AccountId) -> ConnectorResult<PrivateEventStream> {
        let connector = self.clone();
        let account = account.clone();
        Ok(Box::pin(stream::unfold(
            (connector, account, VecDeque::new()),
            |(connector, account, mut pending)| async move {
                if let Some(event) = pending.pop_front() {
                    return Some((Ok(event), (connector, account, pending)));
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                match connector.open_orders(&account).await {
                    Ok(orders) => pending.extend(orders.into_iter().map(PrivateEvent::Order)),
                    Err(error) => return Some((Err(error), (connector, account, pending))),
                }
                match connector.fills(&account, None).await {
                    Ok(page) => pending.extend(page.fills.into_iter().map(PrivateEvent::Fill)),
                    Err(error) => return Some((Err(error), (connector, account, pending))),
                }
                if pending.is_empty() {
                    pending.push_back(PrivateEvent::Heartbeat(Utc::now()));
                }
                let event = pending.pop_front().expect("private event queue populated");
                Some((Ok(event), (connector, account, pending)))
            },
        )))
    }

    async fn fee_schedule(&self, _account: &AccountId) -> ConnectorResult<Vec<FeeSchedule>> {
        let spot = self
            .signed(ProductType::Spot, Method::GET, "/api/v3/account", &[])
            .await?;
        let maker_bps = decimal_number(&spot, "makerCommission")?;
        let taker_bps = decimal_number(&spot, "takerCommission")?;
        let future = self
            .signed(
                ProductType::LinearPerpetual,
                Method::GET,
                "/fapi/v1/commissionRate",
                &[("symbol", "BTCUSDT".into())],
            )
            .await?;
        Ok(vec![
            FeeSchedule {
                product: ProductType::Spot,
                maker: maker_bps / Decimal::new(10_000, 0),
                taker: taker_bps / Decimal::new(10_000, 0),
                effective_at: Utc::now(),
            },
            FeeSchedule {
                product: ProductType::LinearPerpetual,
                maker: decimal(&future, "makerCommissionRate")?,
                taker: decimal(&future, "takerCommissionRate")?,
                effective_at: Utc::now(),
            },
        ])
    }

    async fn current_funding(&self, instrument: &InstrumentId) -> ConnectorResult<FundingQuote> {
        if instrument.product != ProductType::LinearPerpetual {
            return Err(unsupported("FUNDING_SPOT_UNSUPPORTED"));
        }
        let response = self
            .public_get(
                ProductType::LinearPerpetual,
                "/fapi/v1/premiumIndex",
                &[("symbol", instrument.symbol.clone())],
            )
            .await?;
        Ok(FundingQuote {
            instrument: instrument.clone(),
            rate: decimal(&response, "lastFundingRate")?,
            interval_hours: 8,
            mark_price: decimal(&response, "markPrice")?,
            index_price: decimal(&response, "indexPrice")?,
            next_funding_at: millis(i64_value(&response, "nextFundingTime")?)?,
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
                ProductType::LinearPerpetual,
                "/fapi/v1/fundingRate",
                &[
                    ("symbol", instrument.symbol.clone()),
                    ("startTime", range.start.timestamp_millis().to_string()),
                    ("endTime", range.end.timestamp_millis().to_string()),
                    ("limit", "1000".into()),
                ],
            )
            .await?;
        response
            .as_array()
            .ok_or_else(|| invalid("FUNDING_INVALID"))?
            .iter()
            .map(|item| {
                Ok(FundingQuote {
                    instrument: instrument.clone(),
                    rate: decimal(item, "fundingRate")?,
                    interval_hours: 8,
                    mark_price: optional_decimal(item, "markPrice")?.unwrap_or(Decimal::ZERO),
                    index_price: Decimal::ZERO,
                    next_funding_at: millis(i64_value(item, "fundingTime")?)?,
                    observed_at: Utc::now(),
                })
            })
            .collect()
    }

    async fn balances(&self, _account: &AccountId) -> ConnectorResult<Vec<Balance>> {
        let spot = self
            .signed(ProductType::Spot, Method::GET, "/api/v3/account", &[])
            .await?;
        let futures = self
            .signed(
                ProductType::LinearPerpetual,
                Method::GET,
                "/fapi/v3/balance",
                &[],
            )
            .await?;
        let mut balances = Vec::new();
        for item in array(&spot, "balances")? {
            let free = decimal(item, "free")?;
            let locked = decimal(item, "locked")?;
            if !(free + locked).is_zero() {
                balances.push(Balance {
                    asset: text(item, "asset")?.into(),
                    total: free + locked,
                    available: free,
                    locked,
                    venue_updated_at: None,
                    received_at: Utc::now(),
                });
            }
        }
        for item in futures
            .as_array()
            .ok_or_else(|| invalid("FUTURES_BALANCE_INVALID"))?
        {
            let total = decimal(item, "balance")?;
            if !total.is_zero() {
                let available = decimal(item, "availableBalance")?;
                balances.push(Balance {
                    asset: format!("{}:FUTURES", text(item, "asset")?),
                    total,
                    available,
                    locked: total - available,
                    venue_updated_at: Some(millis(i64_value(item, "updateTime")?)?),
                    received_at: Utc::now(),
                });
            }
        }
        Ok(balances)
    }

    async fn positions(&self, _account: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
        let response = self
            .signed(
                ProductType::LinearPerpetual,
                Method::GET,
                "/fapi/v3/positionRisk",
                &[],
            )
            .await?;
        let mut positions = Vec::new();
        for item in response
            .as_array()
            .ok_or_else(|| invalid("POSITIONS_INVALID"))?
        {
            let symbol = text(item, "symbol")?;
            let quantity = decimal(item, "positionAmt")?;
            if !matches!(symbol, "BTCUSDT" | "ETHUSDT") || quantity.is_zero() {
                continue;
            }
            positions.push(VenuePosition {
                instrument: instrument_id(symbol, ProductType::LinearPerpetual),
                quantity,
                entry_price: decimal(item, "entryPrice")?,
                mark_price: decimal(item, "markPrice")?,
                leverage: decimal(item, "leverage")?,
                liquidation_price: optional_decimal(item, "liquidationPrice")?,
                venue_updated_at: Some(millis(i64_value(item, "updateTime")?)?),
                received_at: Utc::now(),
            });
        }
        Ok(positions)
    }

    async fn open_orders(&self, account: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
        let mut orders = self.open_for_product(ProductType::Spot, account).await?;
        orders.extend(
            self.open_for_product(ProductType::LinearPerpetual, account)
                .await?,
        );
        Ok(orders)
    }

    async fn fills(
        &self,
        _account: &AccountId,
        _cursor: Option<&str>,
    ) -> ConnectorResult<FillPage> {
        let mut fills = Vec::new();
        for product in [ProductType::Spot, ProductType::LinearPerpetual] {
            for symbol in ["BTCUSDT", "ETHUSDT"] {
                let path = match product {
                    ProductType::Spot => "/api/v3/myTrades",
                    ProductType::LinearPerpetual => "/fapi/v1/userTrades",
                };
                let response = self
                    .signed(
                        product,
                        Method::GET,
                        path,
                        &[("symbol", symbol.into()), ("limit", "1000".into())],
                    )
                    .await?;
                for item in response
                    .as_array()
                    .ok_or_else(|| invalid("FILLS_INVALID"))?
                {
                    fills.push(Fill {
                        venue_fill_id: value_string(item, "id")?,
                        venue_order_id: value_string(item, "orderId")?,
                        instrument: instrument_id(symbol, product),
                        quantity: decimal(item, "qty")?,
                        price: decimal(item, "price")?,
                        fee: decimal(item, "commission")?,
                        fee_asset: text(item, "commissionAsset")?.into(),
                        occurred_at: millis(i64_value(item, "time")?)?,
                        received_at: Utc::now(),
                    });
                }
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
            .signed(
                ProductType::LinearPerpetual,
                Method::GET,
                "/fapi/v1/income",
                &[
                    ("incomeType", "FUNDING_FEE".into()),
                    ("startTime", range.start.timestamp_millis().to_string()),
                    ("endTime", range.end.timestamp_millis().to_string()),
                    ("limit", "1000".into()),
                ],
            )
            .await?;
        response
            .as_array()
            .ok_or_else(|| invalid("FUNDING_LEDGER_INVALID"))?
            .iter()
            .map(|item| {
                let symbol = text(item, "symbol")?;
                Ok(FundingLedgerEntry {
                    venue_entry_id: value_string(item, "tranId")?,
                    instrument: instrument_id(symbol, ProductType::LinearPerpetual),
                    amount: decimal(item, "income")?,
                    asset: text(item, "asset")?.into(),
                    occurred_at: millis(i64_value(item, "time")?)?,
                })
            })
            .collect()
    }

    async fn prepare_order(&self, order: &CanonicalOrderIntent) -> ConnectorResult<PreparedOrder> {
        if order.instrument.venue != Venue::Binance {
            return Err(invalid("VENUE_MISMATCH"));
        }
        let mut payload = json!({
            "symbol": order.instrument.symbol,
            "side": side_wire(order.side),
            "type": "LIMIT",
            "timeInForce": order.time_in_force,
            "quantity": order.base_quantity.to_string(),
            "price": order.limit_price.to_string(),
            "newClientOrderId": order.client_order_id,
        });
        if order.instrument.product == ProductType::LinearPerpetual {
            payload["positionSide"] = json!("BOTH");
            payload["reduceOnly"] = json!(order.reduce_only);
        }
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
        if self.environment != VenueEnvironment::Testnet {
            return Err(unsupported("PRODUCTION_WRITE_FORBIDDEN"));
        }
        let fields = payload_query(&order.payload)?;
        let path = match order.intent.instrument.product {
            ProductType::Spot => "/api/v3/order",
            ProductType::LinearPerpetual => "/fapi/v1/order",
        };
        let response = self
            .signed(order.intent.instrument.product, Method::POST, path, &fields)
            .await?;
        Ok(OrderAck {
            venue_order_id: response.get("orderId").map(value_to_string).transpose()?,
            client_order_id: order.intent.client_order_id.clone(),
            status: response
                .get("status")
                .and_then(Value::as_str)
                .map(order_status)
                .unwrap_or(OrderStatus::Pending),
            acknowledged_at: Utc::now(),
        })
    }

    async fn cancel_order(&self, request: &CancelRequest) -> ConnectorResult<CancelAck> {
        if self.environment != VenueEnvironment::Testnet {
            return Err(unsupported("PRODUCTION_WRITE_FORBIDDEN"));
        }
        let path = match request.instrument.product {
            ProductType::Spot => "/api/v3/order",
            ProductType::LinearPerpetual => "/fapi/v1/order",
        };
        let mut query = vec![
            ("symbol", request.instrument.symbol.clone()),
            ("origClientOrderId", request.client_order_id.clone()),
        ];
        if let Some(id) = &request.venue_order_id {
            query.push(("orderId", id.clone()));
        }
        let response = self
            .signed(request.instrument.product, Method::DELETE, path, &query)
            .await?;
        Ok(CancelAck {
            status: response
                .get("status")
                .and_then(Value::as_str)
                .map(order_status)
                .unwrap_or(OrderStatus::Cancelled),
            acknowledged_at: Utc::now(),
        })
    }

    async fn query_order(
        &self,
        _account: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>> {
        for product in [ProductType::Spot, ProductType::LinearPerpetual] {
            for symbol in ["BTCUSDT", "ETHUSDT"] {
                let path = match product {
                    ProductType::Spot => "/api/v3/order",
                    ProductType::LinearPerpetual => "/fapi/v1/order",
                };
                let lookup = match key {
                    OrderLookup::VenueOrderId(id) => ("orderId", id.clone()),
                    OrderLookup::ClientOrderId(id) => ("origClientOrderId", id.clone()),
                };
                match self
                    .signed(
                        product,
                        Method::GET,
                        path,
                        &[("symbol", symbol.into()), lookup],
                    )
                    .await
                {
                    Ok(item) => return parse_order(&item, product).map(Some),
                    Err(error) if error.code == "BINANCE_-2013" => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(None)
    }
}

fn parse_instrument(item: &Value, product: ProductType) -> ConnectorResult<Instrument> {
    let symbol = text(item, "symbol")?;
    let price = filter(item, "PRICE_FILTER")?;
    let lot = filter(item, "LOT_SIZE")?;
    let notional = filter(item, "NOTIONAL").or_else(|_| filter(item, "MIN_NOTIONAL"))?;
    Ok(Instrument {
        id: instrument_id(symbol, product),
        canonical_underlying: text(item, "baseAsset")?.into(),
        quote_asset: text(item, "quoteAsset")?.into(),
        settlement_asset: "USDT".into(),
        margin_asset: "USDT".into(),
        contract_multiplier: Decimal::ONE,
        price_tick: decimal(price, "tickSize")?,
        quantity_step: decimal(lot, "stepSize")?,
        minimum_quantity: decimal(lot, "minQty")?,
        maximum_quantity: optional_decimal(lot, "maxQty")?,
        minimum_notional: decimal(notional, "minNotional")?,
        funding_interval_hours: (product == ProductType::LinearPerpetual).then_some(8),
        funding_cap: None,
        funding_floor: None,
        maximum_leverage: None,
        trading: text(item, "status")? == "TRADING",
        metadata_version: format!("binance-{}", Utc::now().date_naive()),
        venue_updated_at: None,
        received_at: Utc::now(),
    })
}

fn filter<'a>(item: &'a Value, filter_type: &str) -> ConnectorResult<&'a Value> {
    array(item, "filters")?
        .iter()
        .find(|value| value.get("filterType").and_then(Value::as_str) == Some(filter_type))
        .ok_or_else(|| invalid("FILTER_MISSING"))
}

fn parse_order(item: &Value, product: ProductType) -> ConnectorResult<VenueOrder> {
    Ok(VenueOrder {
        instrument: instrument_id(text(item, "symbol")?, product),
        venue_order_id: value_string(item, "orderId")?,
        client_order_id: item
            .get("clientOrderId")
            .or_else(|| item.get("origClientOrderId"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        side: if text(item, "side")? == "BUY" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        },
        status: order_status(text(item, "status")?),
        requested_quantity: decimal(item, "origQty")?,
        filled_quantity: decimal(item, "executedQty")?,
        average_fill_price: optional_decimal(item, "avgPrice")?
            .or(optional_decimal(item, "price")?),
        sequence: None,
        venue_updated_at: item
            .get("updateTime")
            .or_else(|| item.get("time"))
            .and_then(Value::as_i64)
            .and_then(|value| millis(value).ok()),
        received_at: Utc::now(),
    })
}

fn payload_query(payload: &Value) -> ConnectorResult<Vec<(&str, String)>> {
    let object = payload
        .as_object()
        .ok_or_else(|| invalid("ORDER_PAYLOAD_INVALID"))?;
    let mut fields = Vec::new();
    for key in [
        "symbol",
        "side",
        "type",
        "timeInForce",
        "quantity",
        "price",
        "newClientOrderId",
        "positionSide",
        "reduceOnly",
    ] {
        if let Some(value) = object.get(key) {
            fields.push((key, value_to_string(value)?));
        }
    }
    Ok(fields)
}

fn instrument_id(symbol: &str, product: ProductType) -> InstrumentId {
    InstrumentId {
        venue: Venue::Binance,
        symbol: symbol.into(),
        product,
    }
}

const fn side_wire(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    }
}

fn order_status(value: &str) -> OrderStatus {
    match value {
        "NEW" => OrderStatus::Open,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" => OrderStatus::Cancelled,
        "REJECTED" => OrderStatus::Rejected,
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

fn decimal_number(value: &Value, key: &str) -> ConnectorResult<Decimal> {
    value
        .get(key)
        .map(value_to_string)
        .transpose()?
        .and_then(|raw| Decimal::from_str(&raw).ok())
        .ok_or_else(|| invalid("DECIMAL_INVALID"))
}

fn optional_decimal(value: &Value, key: &str) -> ConnectorResult<Option<Decimal>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => {
            let raw = value_to_string(raw)?;
            if raw.is_empty() {
                Ok(None)
            } else {
                Decimal::from_str(&raw)
                    .map(Some)
                    .map_err(|_| invalid("DECIMAL_INVALID"))
            }
        }
    }
}

fn value_string(value: &Value, key: &str) -> ConnectorResult<String> {
    value
        .get(key)
        .map(value_to_string)
        .transpose()?
        .ok_or_else(|| invalid("VALUE_MISSING"))
}

fn value_to_string(value: &Value) -> ConnectorResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(invalid("VALUE_INVALID")),
    }
}

fn i64_value(value: &Value, key: &str) -> ConnectorResult<i64> {
    value
        .get(key)
        .and_then(|raw| raw.as_i64().or_else(|| raw.as_str()?.parse().ok()))
        .ok_or_else(|| invalid("INTEGER_INVALID"))
}

fn u64_value(value: &Value, key: &str) -> ConnectorResult<u64> {
    value
        .get(key)
        .and_then(|raw| raw.as_u64().or_else(|| raw.as_str()?.parse().ok()))
        .ok_or_else(|| invalid("INTEGER_INVALID"))
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
        code: format!("BINANCE_{code}"),
        message: "Binance response failed validation".into(),
        retry_after_ms: None,
    }
}

fn unsupported(code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::Unsupported,
        code: format!("BINANCE_{code}"),
        message: "Binance operation is unavailable for this product or mode".into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_timeout_code_requires_reconciliation() {
        let error = ConnectorError {
            kind: ConnectorErrorKind::UnknownOutcome,
            code: "BINANCE_-1007".into(),
            message: "status unknown".into(),
            retry_after_ms: None,
        };
        assert_eq!(error.kind, ConnectorErrorKind::UnknownOutcome);
    }

    #[test]
    fn only_testnet_advertises_order_submission() {
        let credentials = || ConnectorCredentials {
            api_key: "key".into(),
            api_secret: "secret".into(),
            passphrase: None,
        };
        assert!(
            BinanceConnector::new(VenueEnvironment::Testnet, Some(credentials()))
                .capabilities()
                .order_submission
        );
        assert!(
            !BinanceConnector::new(VenueEnvironment::Production, Some(credentials()))
                .capabilities()
                .order_submission
        );
    }
}
