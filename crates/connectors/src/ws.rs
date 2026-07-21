//! Shared JSON WebSocket transport with bounded buffering and reconnects.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt, stream};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{ConnectorError, ConnectorErrorKind, ConnectorResult};

const CHANNEL_CAPACITY: usize = 256;
const SEND_DEADLINE: Duration = Duration::from_secs(2);
const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) struct JsonWsConfig {
    pub name: &'static str,
    pub url: String,
    pub initial_messages: Vec<Value>,
    /// Venue application-level heartbeat. Protocol ping frames are answered
    /// automatically by tungstenite.
    pub heartbeat: Option<String>,
    pub inactivity_timeout: Duration,
}

pub(crate) fn connect_json(
    config: JsonWsConfig,
) -> impl futures_core::Stream<Item = ConnectorResult<Value>> + Send + 'static {
    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::spawn(run(config, sender));
    stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    })
}

async fn run(config: JsonWsConfig, sender: mpsc::Sender<ConnectorResult<Value>>) {
    let mut reconnect_delay = RECONNECT_MIN;
    loop {
        if sender.is_closed() {
            return;
        }
        let connection = connect_async(&config.url).await;
        let Ok((socket, _)) = connection else {
            if !send(&sender, Err(transport_error(config.name, "CONNECT_FAILED"))).await {
                return;
            }
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(RECONNECT_MAX);
            continue;
        };

        reconnect_delay = RECONNECT_MIN;
        let (mut writer, mut reader) = socket.split();
        let mut initial_failed = false;
        for message in &config.initial_messages {
            let Ok(encoded) = serde_json::to_string(message) else {
                initial_failed = true;
                break;
            };
            if writer.send(Message::Text(encoded.into())).await.is_err() {
                initial_failed = true;
                break;
            }
        }
        if initial_failed {
            if !send(
                &sender,
                Err(transport_error(config.name, "SUBSCRIBE_FAILED")),
            )
            .await
            {
                return;
            }
            continue;
        }
        if !send(
            &sender,
            Ok(serde_json::json!({"_perpeto_transport": "connected"})),
        )
        .await
        {
            return;
        }

        loop {
            let next = tokio::time::timeout(config.inactivity_timeout, reader.next()).await;
            match next {
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<Value>(&text) {
                    Ok(value) => {
                        if !send(&sender, Ok(value)).await {
                            return;
                        }
                    }
                    Err(_) if text.as_str() == "pong" => {}
                    Err(_) => {
                        if !send(
                            &sender,
                            Err(invalid_error(config.name, "INVALID_JSON_FRAME")),
                        )
                        .await
                        {
                            return;
                        }
                    }
                },
                Ok(Some(Ok(Message::Binary(_)))) => {
                    if !send(
                        &sender,
                        Err(invalid_error(config.name, "UNSUPPORTED_BINARY_FRAME")),
                    )
                    .await
                    {
                        return;
                    }
                }
                Ok(Some(Ok(Message::Ping(payload)))) => {
                    if writer.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Ok(Some(Ok(Message::Pong(_)))) => {}
                Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) => break,
                Ok(Some(Ok(Message::Frame(_)))) => {}
                Err(_) => {
                    let heartbeat = config
                        .heartbeat
                        .clone()
                        .map(|text| Message::Text(text.into()))
                        .unwrap_or_else(|| Message::Ping(Vec::new().into()));
                    if writer.send(heartbeat).await.is_err() {
                        break;
                    }
                    match tokio::time::timeout(Duration::from_secs(10), reader.next()).await {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if text.as_str() != "pong"
                                && let Ok(value) = serde_json::from_str::<Value>(&text)
                                && !send(&sender, Ok(value)).await
                            {
                                return;
                            }
                        }
                        Ok(Some(Ok(Message::Pong(_)))) => {}
                        _ => break,
                    }
                }
            }
        }

        if !send(
            &sender,
            Err(ConnectorError {
                kind: ConnectorErrorKind::ReconciliationRequired,
                code: format!("{}_STREAM_DISCONNECTED", config.name),
                message: format!(
                    "{} stream disconnected; reconciliation is required",
                    config.name
                ),
                retry_after_ms: None,
            }),
        )
        .await
        {
            return;
        }
        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = (reconnect_delay * 2).min(RECONNECT_MAX);
    }
}

async fn send(sender: &mpsc::Sender<ConnectorResult<Value>>, item: ConnectorResult<Value>) -> bool {
    matches!(
        tokio::time::timeout(SEND_DEADLINE, sender.send(item)).await,
        Ok(Ok(()))
    )
}

fn transport_error(name: &str, code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::Transient,
        code: format!("{name}_{code}"),
        message: format!("{name} WebSocket transport is unavailable"),
        retry_after_ms: None,
    }
}

fn invalid_error(name: &str, code: &str) -> ConnectorError {
    ConnectorError {
        kind: ConnectorErrorKind::InvalidResponse,
        code: format!("{name}_{code}"),
        message: format!("{name} WebSocket returned an invalid frame"),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::*;

    #[tokio::test]
    async fn sends_subscription_and_yields_json_without_exposing_url() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("upgrade");
            let first = socket.next().await.expect("message").expect("valid");
            assert_eq!(first.into_text().expect("text"), r#"{"op":"subscribe"}"#);
            socket
                .send(Message::Text(r#"{"topic":"book"}"#.into()))
                .await
                .expect("send");
        });

        let mut stream = Box::pin(connect_json(JsonWsConfig {
            name: "TEST",
            url: format!("ws://{address}/private-listen-key"),
            initial_messages: vec![json!({"op": "subscribe"})],
            heartbeat: None,
            inactivity_timeout: Duration::from_secs(2),
        }));
        let connected = stream.next().await.expect("connected").expect("valid");
        assert_eq!(connected["_perpeto_transport"], "connected");
        let event = stream.next().await.expect("event").expect("valid");
        assert_eq!(event["topic"], "book");
    }
}
