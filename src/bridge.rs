//! Mensagens entre a interface (thread principal) e o backend assíncrono (tokio).
use crate::model::{ChatMessage, Snapshot, User};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

pub enum LoginMode {
    Login,
    Register { invite: String },
    Setup { setup_key: String },
}

pub enum Command {
    Admin {
        method: reqwest::Method,
        path: String,
        body: Option<serde_json::Value>,
    },
    /// Testa o servidor e informa se ele ainda precisa do primeiro administrador.
    Probe {
        server: String,
    },
    Login {
        server: String,
        username: String,
        password: String,
        mode: LoginMode,
    },
    Logout,
    LoadMessages {
        channel: String,
        before: Option<i64>,
    },
    SendMessage {
        channel: String,
        content: String,
    },
    JoinVoice {
        channel: String,
    },
    LeaveVoice,
    SetMuted(bool),
    SetDeafened(bool),
    SetVolume {
        user: String,
        kind: crate::audio::AudioKind,
        volume: crate::audio::Volume,
    },
    ListSources {
        request: u64,
    },
    StartScreen(crate::capture::ShareRequest),
    StopScreen,
    ListDevices,
    ListCameras,
    StartCamera(Device),
    StopCamera,
    SelectMic(String),
    SelectSpeaker(String),
    SetProcessing {
        echo: bool,
        agc: bool,
        noise: bool,
    },
    DemoState(VoiceStatus),
    DemoError,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Clone)]
pub struct ParticipantView {
    pub identity: String,
    pub name: String,
    pub local: bool,
    pub speaking: bool,
    pub muted: bool,
    pub volume: crate::audio::ParticipantVolume,
    pub screen_audio: bool,
}

#[derive(Clone)]
pub struct TileView {
    /// SID da faixa de vídeo; chave em [`Frames`].
    pub key: String,
    pub name: String,
    pub screen: bool,
    pub user: String,
}

#[derive(Clone)]
pub struct VoiceView {
    pub channel: String,
    pub status: VoiceStatus,
    pub muted: bool,
    pub deafened: bool,
    pub screen: bool,
    pub screen_pending: bool,
    pub camera: bool,
    pub camera_pending: bool,
    pub share_video: bool,
    pub share_audio: bool,
    pub participants: Vec<ParticipantView>,
    pub tiles: Vec<TileView>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

pub enum Event {
    AdminFinished(Result<(), String>),
    Cameras(Result<Vec<Device>, String>),
    Sources {
        request: u64,
        result: Result<Vec<crate::capture::CaptureSourceInfo>, String>,
    },
    Probed {
        needs_setup: bool,
    },
    LoggedIn {
        user: User,
    },
    /// Sessão salva inválida ou login recusado; volta à tela de entrada.
    LoggedOut {
        error: Option<String>,
    },
    /// A conta salva continua; o servidor ainda não respondeu e o backend tenta de novo.
    Reconnecting {
        error: String,
    },
    Connection(bool),
    Snapshot(Snapshot),
    Presence {
        online: Vec<String>,
        voice: Vec<crate::model::VoicePresence>,
    },
    Message(ChatMessage),
    Messages {
        channel: String,
        messages: Vec<ChatMessage>,
        older: bool,
    },
    Voice(Option<VoiceView>),
    Devices {
        mics: Vec<Device>,
        speakers: Vec<Device>,
    },
    Error(String),
}

/// Último frame RGBA de cada faixa de vídeo; o vídeo é escrito pelas tarefas de
/// recepção e lido pela interface, que só reenvia a textura quando `seq` muda.
pub struct RgbaFrame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    pub seq: u64,
}

pub type Frames = Arc<Mutex<HashMap<String, RgbaFrame>>>;

/// Envia eventos para a interface e pede um novo quadro ao egui.
#[derive(Clone)]
pub struct UiSink {
    tx: mpsc::Sender<Event>,
    ctx: eframe::egui::Context,
}

impl UiSink {
    pub fn new(tx: mpsc::Sender<Event>, ctx: eframe::egui::Context) -> Self {
        Self { tx, ctx }
    }

    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }

    pub fn repaint(&self) {
        self.ctx.request_repaint();
    }

    pub fn error(&self, message: impl Into<String>) {
        self.send(Event::Error(message.into()));
    }
}
