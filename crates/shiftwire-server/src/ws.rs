//! Live push to the UI: one WebSocket per open window, fed by the core event bus.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use serde::Deserialize;
use shiftwire_core::{Event, Filter};

use crate::server::Ctx;

pub async fn upgrade(ws: WebSocketUpgrade, State(c): State<Ctx>) -> Response {
    ws.on_upgrade(move |socket| session(socket, c))
}

async fn session(mut socket: WebSocket, c: Ctx) {
    let mut events = c.app.subscribe();
    let mut stop = c.stop.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    // Each window follows only the flows matching its own filter.
    let mut filter = Filter::default();
    if send(
        &mut socket,
        &json!({ "type": "hello", "status": c.app.status() }),
    )
    .await
    .is_err()
    {
        return;
    }
    loop {
        let sent = tokio::select! {
            event = events.recv() => match event {
                // History gets what matches its filter; the AI Timeline needs every agent request.
                Ok(Event::FlowAdded { flow }) => {
                    let matches = filter.matches(&flow);
                    let agent = flow.actor != "proxy" && flow.actor != "human";
                    if matches || agent {
                        send(&mut socket, &json!({ "type": "flow_added", "flow": flow, "matches": matches })).await
                    } else {
                        Ok(())
                    }
                }
                Ok(Event::FlowUpdated { flow }) => {
                    let matches = filter.matches(&flow);
                    send(&mut socket, &json!({ "type": "flow_updated", "flow": flow, "matches": matches })).await
                }
                Ok(event) => send(&mut socket, &event).await,
                // The UI fell behind; it refetches instead of us buffering forever.
                Err(RecvError::Lagged(missed)) => send(&mut socket, &json!({ "type": "resync", "missed": missed })).await,
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Subscribe { filter: text }) => match Filter::parse(&text) {
                        Ok(parsed) => {
                            filter = parsed;
                            send(&mut socket, &json!({ "type": "subscribed", "filter": text })).await
                        }
                        Err(e) => send(&mut socket, &json!({ "type": "filter_error", "message": e.0 })).await,
                    },
                    Err(_) => Ok(()), // ignore unknown messages
                },
                Some(Ok(_)) => Ok(()),
            },
            _ = ping.tick() => socket.send(Message::Ping(Bytes::new())).await,
            () = stopped(&mut stop) => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
        };
        if sent.is_err() {
            break;
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Subscribe { filter: String },
}

/// Resolves once shutdown starts (drops the watch guard so the future stays `Send`).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|s| *s).await;
}

async fn send<T: Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).expect("events always serialize");
    socket.send(Message::Text(text.into())).await
}
