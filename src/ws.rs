//! WebSocket `/api/ws`: recebe eventos do servidor e envia o estado de mídia.
//! Reconecta com backoff e jitter limitado a 15 s, como o cliente web.
use crate::api::Api;
use crate::model::{Envelope, ServerEvent};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest, http};

pub enum WsEvent {
    Connected(bool),
    Server(ServerEvent),
    /// O upgrade foi recusado por autenticação: a sessão não vale mais.
    Unauthorized,
}

/// Envia o evento de mídia ao servidor e o reenvia após cada reconexão.
#[derive(Clone, Default)]
pub struct PresenceOut {
    last: Arc<Mutex<Option<String>>>,
    tx: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl PresenceOut {
    pub fn set(&self, json: Option<String>) {
        if *self.last.lock().unwrap() == json {
            return;
        }
        *self.last.lock().unwrap() = json.clone();
        if let (Some(json), Some(tx)) = (json, self.tx.lock().unwrap().as_ref()) {
            let _ = tx.send(json);
        }
    }
}

pub fn spawn(api: Api, presence: PresenceOut, events: mpsc::UnboundedSender<WsEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            match session(&api, &presence, &events).await {
                Err(Stop) => return,
                Ok(connected) => {
                    let _ = events.send(WsEvent::Connected(false));
                    attempt = if connected { 0 } else { attempt + 1 };
                }
            }
            let base = 500u64.saturating_mul(1 << attempt.min(5)).min(15_000);
            let jitter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_millis() as u64 % 1000)
                .unwrap_or(0);
            tokio::time::sleep(Duration::from_millis((base + jitter).min(15_000))).await;
        }
    })
}

struct Stop;

/// Uma conexão completa; `Ok(true)` se chegou a conectar antes de cair.
async fn session(api: &Api, presence: &PresenceOut, events: &mpsc::UnboundedSender<WsEvent>) -> Result<bool, Stop> {
    let url = format!("{}/api/ws", api.origin().replacen("http", "ws", 1));
    let Ok(mut request) = url.into_client_request() else { return Err(Stop) };
    let headers = request.headers_mut();
    headers.insert(http::header::ORIGIN, api.origin().parse().map_err(|_| Stop)?);
    if let Some(cookie) = api.cookie_header().and_then(|c| c.parse().ok()) {
        headers.insert(http::header::COOKIE, cookie);
    }
    let stream = match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _)) => stream,
        Err(tungstenite::Error::Http(res)) if matches!(res.status().as_u16(), 401 | 403) => {
            let _ = events.send(WsEvent::Unauthorized);
            return Err(Stop);
        }
        Err(e) => {
            log::info!("websocket indisponível: {e}");
            return Ok(false);
        }
    };
    let _ = events.send(WsEvent::Connected(true));
    let (mut sink, mut source) = stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel();
    *presence.tx.lock().unwrap() = Some(tx);
    let pending = presence.last.lock().unwrap().clone();
    if let Some(json) = pending {
        let _ = sink.send(Message::text(json)).await;
    }
    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<Envelope>(&text) {
                    Ok(env) if env.v == 1 => {
                        if events.send(WsEvent::Server(env.event)).is_err() {
                            return Err(Stop);
                        }
                    }
                    _ => log::debug!("evento ignorado"),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            outgoing = rx.recv() => match outgoing {
                Some(json) => if sink.send(Message::text(json)).await.is_err() { break },
                None => break,
            },
        }
    }
    *presence.tx.lock().unwrap() = None;
    Ok(true)
}
