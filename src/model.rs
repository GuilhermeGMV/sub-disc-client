//! Tipos do contrato HTTP/WebSocket v1 do servidor (ver docs/api.md do Subdisc).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Text,
    Voice,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub channel_id: String,
    pub username: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VoicePresence {
    pub user_id: String,
    pub channel_id: String,
    pub connected: bool,
    pub muted: bool,
    pub screen: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    pub users: Vec<User>,
    pub channels: Vec<Channel>,
    pub online: Vec<String>,
    pub voice: Vec<VoicePresence>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Snapshot {
        data: Snapshot,
    },
    Message {
        message: ChatMessage,
    },
    Presence {
        online: Vec<String>,
        voice: Vec<VoicePresence>,
    },
    ChannelsChanged,
    UsersChanged,
    VoiceReset,
}

/// Envelope `{"v":1,...}`; outras versões são ignoradas.
#[derive(Deserialize)]
pub struct Envelope {
    pub v: u32,
    #[serde(flatten)]
    pub event: ServerEvent,
}

#[derive(Debug, Deserialize)]
pub struct JoinResponse {
    pub url: String,
    pub token: String,
    pub lease: String,
}

#[derive(Serialize)]
pub struct MediaEvent<'a> {
    pub v: u32,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub lease: &'a str,
    pub muted: bool,
    pub camera: bool,
    pub screen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_events() {
        let message = r#"{"v":1,"type":"message","message":{"id":42,"channel_id":"c","user_id":"u","username":"ana","content":"oi","created_at":1792360000}}"#;
        let env: Envelope = serde_json::from_str(message).unwrap();
        assert!(
            matches!(env.event, ServerEvent::Message { message } if message.id == 42 && message.content == "oi")
        );
        for raw in [
            r#"{"v":1,"type":"presence","online":["u"],"voice":[]}"#,
            r#"{"v":1,"type":"channels_changed"}"#,
            r#"{"v":1,"type":"voice_reset"}"#,
            r#"{"v":1,"type":"snapshot","data":{"users":[{"id":"u","username":"ana","role":"admin"}],"channels":[{"id":"c","name":"geral","kind":"text","created_at":1}],"online":[],"voice":[{"user_id":"u","channel_id":"v","identity":"u:l","lease":"l","connected":true,"muted":false,"camera":false,"screen":true}]}}"#,
        ] {
            let env: Envelope = serde_json::from_str(raw).unwrap();
            assert_eq!(env.v, 1);
        }
    }

    #[test]
    fn media_event_matches_contract() {
        let event = MediaEvent {
            v: 1,
            kind: "media",
            lease: "l",
            muted: true,
            camera: false,
            screen: false,
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"v":1,"type":"media","lease":"l","muted":true,"camera":false,"screen":false}"#
        );
    }
}
