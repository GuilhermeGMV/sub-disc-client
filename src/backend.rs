//! Laço assíncrono que executa os comandos da interface: sessão, WebSocket de
//! eventos, histórico de mensagens e a chamada de voz.
use crate::api::{Api, normalize_origin};
use crate::bridge::{Command, Device, Event, Frames, LoginMode, UiSink};
use crate::config::Config;
use crate::model::ServerEvent;
use crate::voice::{self, VoiceCmd, VoiceHandle, VoiceSetup};
use crate::ws::{self, PresenceOut, WsEvent};
use livekit::{PlatformAudio, PlayoutDeviceId, RecordingDeviceId};
use reqwest::StatusCode;
use std::collections::HashSet;
use tokio::sync::mpsc;

struct Backend {
    ui: UiSink,
    frames: Frames,
    config: Config,
    api: Option<Api>,
    ws: Option<tokio::task::JoinHandle<()>>,
    ws_tx: mpsc::UnboundedSender<WsEvent>,
    presence: PresenceOut,
    voice: Option<VoiceHandle>,
    audio: Option<PlatformAudio>,
    deafened: bool,
    locally_muted: HashSet<String>,
}

pub async fn run(mut commands: mpsc::UnboundedReceiver<Command>, ui: UiSink, frames: Frames) {
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel();
    let mut backend = Backend {
        ui,
        frames,
        config: Config::load(),
        api: None,
        ws: None,
        ws_tx,
        presence: PresenceOut::default(),
        voice: None,
        audio: None,
        deafened: false,
        locally_muted: HashSet::new(),
    };
    backend.resume().await;
    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(cmd) => backend.command(cmd).await,
                None => break,
            },
            Some(event) = ws_rx.recv() => backend.ws_event(event).await,
        }
    }
    backend.leave_voice().await;
}

impl Backend {
    /// Reabre a sessão salva, se ainda for válida.
    async fn resume(&mut self) {
        let Some(session) = self.config.session.clone() else {
            self.ui.send(Event::LoggedOut { error: None });
            return;
        };
        let api = Api::new(self.config.server.clone(), Some(session));
        match api.me().await {
            Ok(user) => self.start_session(api, user),
            Err(e) => {
                if e.status == Some(StatusCode::UNAUTHORIZED) {
                    self.config.session = None;
                    self.config.save();
                }
                self.ui.send(Event::LoggedOut { error: Some(e.message) });
            }
        }
    }

    fn start_session(&mut self, api: Api, user: crate::model::User) {
        self.config.server = api.origin().to_owned();
        self.config.username = user.username.clone();
        self.config.session = api.session();
        self.config.save();
        self.ws = Some(ws::spawn(api.clone(), self.presence.clone(), self.ws_tx.clone()));
        self.api = Some(api);
        self.ui.send(Event::LoggedIn { user });
    }

    async fn end_session(&mut self, error: Option<String>) {
        self.leave_voice().await;
        if let Some(ws) = self.ws.take() {
            ws.abort();
        }
        self.api = None;
        self.config.session = None;
        self.config.save();
        self.ui.send(Event::LoggedOut { error });
    }

    async fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Probe { server } => match normalize_origin(&server) {
                Ok(origin) => match Api::new(origin, None).needs_setup().await {
                    Ok(needs_setup) => self.ui.send(Event::Probed { needs_setup }),
                    Err(e) => self.ui.error(e.message),
                },
                Err(e) => self.ui.error(e),
            },
            Command::Login { server, username, password, mode } => {
                let origin = match normalize_origin(&server) {
                    Ok(o) => o,
                    Err(e) => return self.ui.send(Event::LoggedOut { error: Some(e) }),
                };
                let api = Api::new(origin, None);
                let result = match &mode {
                    LoginMode::Login => api.login(&username, &password).await,
                    LoginMode::Register { invite } => api.register(&username, &password, invite.trim()).await,
                    LoginMode::Setup { setup_key } => api.setup(&username, &password, setup_key.trim()).await,
                };
                match result {
                    Ok(user) if api.session().is_some() => self.start_session(api, user),
                    Ok(_) => self.ui.send(Event::LoggedOut { error: Some("O servidor não abriu a sessão.".into()) }),
                    Err(e) => self.ui.send(Event::LoggedOut { error: Some(e.message) }),
                }
            }
            Command::Logout => {
                if let Some(api) = &self.api {
                    let _ = api.logout().await;
                }
                self.end_session(None).await;
            }
            Command::LoadMessages { channel, before } => {
                let Some(api) = self.api.clone() else { return };
                let ui = self.ui.clone();
                tokio::spawn(async move {
                    match api.messages(&channel, before).await {
                        Ok(messages) => ui.send(Event::Messages { channel, messages, older: before.is_some() }),
                        Err(e) => ui.error(e.message),
                    }
                });
            }
            Command::SendMessage { channel, content } => {
                let Some(api) = self.api.clone() else { return };
                let ui = self.ui.clone();
                tokio::spawn(async move {
                    match api.send_message(&channel, &content).await {
                        Ok(message) => ui.send(Event::Message(message)),
                        Err(e) => ui.error(e.message),
                    }
                });
            }
            Command::JoinVoice { channel } => self.join_voice(channel).await,
            Command::LeaveVoice => self.leave_voice().await,
            Command::SetMuted(muted) => self.voice_cmd(VoiceCmd::SetMuted(muted)),
            Command::SetDeafened(deafened) => {
                self.deafened = deafened;
                self.voice_cmd(VoiceCmd::SetDeafened(deafened));
            }
            Command::MuteParticipant { identity, muted } => {
                let user = identity.split(':').next().unwrap_or(&identity).to_owned();
                if muted {
                    self.locally_muted.insert(user);
                } else {
                    self.locally_muted.remove(&user);
                }
                self.voice_cmd(VoiceCmd::MuteParticipant { identity, muted });
            }
            Command::StartScreen => self.voice_cmd(VoiceCmd::StartScreen),
            Command::StopScreen => self.voice_cmd(VoiceCmd::StopScreen),
            Command::ListDevices => {
                self.platform_audio();
            }
            Command::SelectMic(id) => {
                if let Some(audio) = self.platform_audio() {
                    let id = RecordingDeviceId::from_unchecked_guid(&id);
                    let result = if self.voice.is_some() { audio.switch_recording_device(&id) } else { audio.set_recording_device(&id) };
                    if result.is_err() {
                        self.ui.error("Não foi possível trocar o microfone.");
                    }
                }
            }
            Command::SelectSpeaker(id) => {
                if let Some(audio) = self.platform_audio() {
                    let id = PlayoutDeviceId::from_unchecked_guid(&id);
                    let result = if self.voice.is_some() { audio.switch_playout_device(&id) } else { audio.set_playout_device(&id) };
                    if result.is_err() {
                        self.ui.error("Não foi possível trocar a saída de áudio.");
                    }
                }
            }
            Command::SetProcessing { echo, agc, noise } => {
                if let Some(audio) = self.platform_audio() {
                    let ok = audio.set_echo_cancellation(echo, false).is_ok()
                        && audio.set_auto_gain_control(agc, false).is_ok()
                        && audio.set_noise_suppression(noise, false).is_ok();
                    if !ok {
                        self.ui.error("Não foi possível aplicar o processamento de áudio.");
                    }
                }
            }
        }
    }

    /// Abre o áudio nativo na primeira necessidade e publica a lista de dispositivos.
    fn platform_audio(&mut self) -> Option<PlatformAudio> {
        if self.audio.is_none() {
            match PlatformAudio::new() {
                Ok(audio) => {
                    let device = |id: &str, name: &str| Device { id: id.to_owned(), name: name.to_owned() };
                    let mics = audio.recording_devices().map(|d| device(d.id.as_str(), &d.name)).collect();
                    let speakers = audio.playout_devices().map(|d| device(d.id.as_str(), &d.name)).collect();
                    self.ui.send(Event::Devices { mics, speakers });
                    self.audio = Some(audio);
                }
                Err(e) => {
                    log::warn!("áudio indisponível: {e}");
                    self.ui.error("Nenhum dispositivo de áudio disponível.");
                }
            }
        }
        self.audio.clone()
    }

    async fn join_voice(&mut self, channel: String) {
        self.leave_voice().await;
        let (Some(api), Some(audio)) = (self.api.clone(), self.platform_audio()) else { return };
        self.voice = Some(voice::spawn(VoiceSetup {
            api,
            channel,
            audio,
            ui: self.ui.clone(),
            frames: self.frames.clone(),
            presence: self.presence.clone(),
            deafened: self.deafened,
            locally_muted: self.locally_muted.clone(),
        }));
    }

    async fn leave_voice(&mut self) {
        if let Some(voice) = self.voice.take() {
            let _ = voice.tx.send(VoiceCmd::Leave);
            let _ = voice.task.await;
        }
    }

    fn voice_cmd(&mut self, cmd: VoiceCmd) {
        if let Some(voice) = &self.voice {
            if voice.task.is_finished() {
                self.voice = None;
            } else {
                let _ = voice.tx.send(cmd);
            }
        }
    }

    async fn ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Connected(connected) => self.ui.send(Event::Connection(connected)),
            WsEvent::Unauthorized => self.end_session(Some("Sessão expirada. Entre novamente.".into())).await,
            WsEvent::Server(event) => match event {
                ServerEvent::Snapshot { data } => self.ui.send(Event::Snapshot(data)),
                ServerEvent::Message { message } => self.ui.send(Event::Message(message)),
                ServerEvent::Presence { online, voice } => self.ui.send(Event::Presence { online, voice }),
                ServerEvent::ChannelsChanged | ServerEvent::UsersChanged => {
                    let Some(api) = self.api.clone() else { return };
                    match api.state().await {
                        Ok(state) => self.ui.send(Event::Snapshot(state)),
                        Err(e) => self.ui.error(e.message),
                    }
                }
                // O backend reiniciou e esqueceu as chamadas: é preciso entrar de novo.
                ServerEvent::VoiceReset => {
                    if self.voice.is_some() {
                        self.leave_voice().await;
                        self.ui.error("A chamada foi reiniciada pelo servidor. Entre novamente.");
                    }
                }
            },
        }
    }
}
