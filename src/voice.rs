//! Sessão de voz: entra na sala LiveKit com o token do servidor, publica o
//! microfone pelo áudio nativo do WebRTC (que também toca o áudio recebido),
//! recebe vídeos/telas e compartilha a tela.
use crate::api::Api;
use crate::bridge::{Event, Frames, ParticipantView, RgbaFrame, TileView, UiSink, VoiceStatus, VoiceView};
use crate::model::MediaEvent;
use crate::screen::ScreenShare;
use crate::ws::PresenceOut;
use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use livekit::PlatformAudio;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub enum VoiceCmd {
    SetMuted(bool),
    SetDeafened(bool),
    MuteParticipant { identity: String, muted: bool },
    StartScreen,
    StopScreen,
    Leave,
}

pub struct VoiceHandle {
    pub tx: mpsc::UnboundedSender<VoiceCmd>,
    pub task: JoinHandle<()>,
}

pub struct VoiceSetup {
    pub api: Api,
    pub channel: String,
    pub audio: PlatformAudio,
    pub ui: UiSink,
    pub frames: Frames,
    pub presence: PresenceOut,
    pub deafened: bool,
    /// Usuários silenciados localmente, por ID (a identidade LiveKit é `usuario:lease`).
    pub locally_muted: HashSet<String>,
}

pub fn spawn(setup: VoiceSetup) -> VoiceHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(setup, rx));
    VoiceHandle { tx, task }
}

fn user_of(identity: &str) -> &str {
    identity.split(':').next().unwrap_or(identity)
}

struct Session {
    setup: VoiceSetup,
    room: Room,
    lease: String,
    mic: LocalAudioTrack,
    status: VoiceStatus,
    muted: bool,
    screen: Option<(ScreenShare, TrackSid)>,
    /// Seletor de tela aberto; o resultado chega por `screen_rx` sem travar o laço.
    screen_pending: bool,
    screen_tx: mpsc::UnboundedSender<ScreenResult>,
    videos: HashMap<String, (JoinHandle<()>, TileView)>,
}

async fn run(setup: VoiceSetup, mut rx: mpsc::UnboundedReceiver<VoiceCmd>) {
    let ui = setup.ui.clone();
    ui.send(Event::Voice(Some(VoiceView {
        channel: setup.channel.clone(),
        status: VoiceStatus::Connecting,
        muted: false,
        deafened: setup.deafened,
        screen: false,
        participants: vec![],
        tiles: vec![],
    })));
    let (screen_tx, mut screen_rx) = mpsc::unbounded_channel();
    let (mut session, mut events) = match connect(setup, screen_tx).await {
        Ok(v) => v,
        Err(e) => {
            ui.error(e);
            ui.send(Event::Voice(None));
            return;
        }
    };
    session.publish();
    // Detecta capturas de tela encerradas fora do app (seletor cancelado etc.).
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if !session.screen.as_ref().is_some_and(|(share, _)| share.finished()) {
                    continue;
                }
            }
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    log::info!("sala encerrada: {reason:?}");
                    ui.error("A chamada foi encerrada. Você pode entrar novamente.");
                    break;
                }
                Some(event) => session.handle(event),
                None => break,
            },
            Some(result) = screen_rx.recv() => {
                session.screen_pending = false;
                match result {
                    Ok(share) => session.screen = Some(share),
                    Err(e) => session.setup.ui.error(e),
                }
            }
            cmd = rx.recv() => match cmd {
                Some(VoiceCmd::Leave) | None => break,
                Some(cmd) => session.command(cmd).await,
            },
        }
        session.publish();
    }
    session.close().await;
}

type ScreenResult = Result<(ScreenShare, TrackSid), String>;

async fn connect(
    setup: VoiceSetup,
    screen_tx: mpsc::UnboundedSender<ScreenResult>,
) -> Result<(Session, mpsc::UnboundedReceiver<RoomEvent>), String> {
    let join = setup.api.join(&setup.channel).await.map_err(|e| e.to_string())?;
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.dynacast = true;
    let (room, events) = match Room::connect(&join.url, &join.token, options).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("falha ao conectar na sala: {e}");
            let _ = setup.api.leave(&join.lease).await;
            return Err("Não foi possível conectar à chamada.".into());
        }
    };
    let mic = LocalAudioTrack::create_audio_track("microphone", setup.audio.rtc_source());
    let options = TrackPublishOptions {
        source: TrackSource::Microphone,
        audio_encoding: Some(AudioEncoding { max_bitrate: 96_000 }),
        dtx: true,
        red: true,
        ..Default::default()
    };
    if let Err(e) = room.local_participant().publish_track(LocalTrack::Audio(mic.clone()), options).await {
        log::warn!("falha ao publicar o microfone: {e}");
        setup.ui.error("Não foi possível abrir o microfone.");
    }
    let mut session = Session {
        setup,
        room,
        lease: join.lease,
        mic,
        status: VoiceStatus::Connected,
        muted: false,
        screen: None,
        screen_pending: false,
        screen_tx,
        videos: HashMap::new(),
    };
    // Faixas já assinadas antes do loop de eventos começar.
    for participant in session.room.remote_participants().into_values() {
        for publication in participant.track_publications().into_values() {
            if let Some(track) = publication.track() {
                session.attach(track, &participant);
            }
        }
    }
    Ok((session, events))
}

impl Session {
    fn handle(&mut self, event: RoomEvent) {
        match event {
            RoomEvent::TrackSubscribed { track, participant, .. } => self.attach(track, &participant),
            RoomEvent::TrackUnsubscribed { track, .. } => self.detach(&track.sid()),
            RoomEvent::Reconnecting => self.status = VoiceStatus::Reconnecting,
            RoomEvent::Reconnected => self.status = VoiceStatus::Connected,
            _ => {}
        }
    }

    fn audio_enabled(&self, identity: &str) -> bool {
        !self.setup.deafened && !self.setup.locally_muted.contains(user_of(identity))
    }

    fn attach(&mut self, track: RemoteTrack, participant: &RemoteParticipant) {
        match track {
            RemoteTrack::Audio(audio) => {
                if self.audio_enabled(participant.identity().as_str()) {
                    audio.enable();
                } else {
                    audio.disable();
                }
            }
            RemoteTrack::Video(video) => {
                let key = video.sid().to_string();
                let tile = TileView {
                    key: key.clone(),
                    name: participant.name(),
                    screen: video.source() == TrackSource::Screenshare,
                };
                let task = tokio::spawn(receive_video(video, key.clone(), self.setup.frames.clone(), self.setup.ui.clone()));
                if let Some((old, _)) = self.videos.insert(key, (task, tile)) {
                    old.abort();
                }
            }
        }
    }

    fn detach(&mut self, sid: &TrackSid) {
        let key = sid.to_string();
        if let Some((task, _)) = self.videos.remove(&key) {
            task.abort();
        }
        self.setup.frames.lock().unwrap().remove(&key);
    }

    fn apply_audio(&self) {
        for participant in self.room.remote_participants().into_values() {
            let enabled = self.audio_enabled(participant.identity().as_str());
            for publication in participant.track_publications().into_values() {
                if let Some(RemoteTrack::Audio(audio)) = publication.track() {
                    if enabled { audio.enable() } else { audio.disable() }
                }
            }
        }
    }

    async fn command(&mut self, cmd: VoiceCmd) {
        match cmd {
            VoiceCmd::SetMuted(muted) => {
                self.muted = muted;
                if muted { self.mic.mute() } else { self.mic.unmute() }
            }
            VoiceCmd::SetDeafened(deafened) => {
                self.setup.deafened = deafened;
                self.apply_audio();
            }
            VoiceCmd::MuteParticipant { identity, muted } => {
                let user = user_of(&identity).to_owned();
                if muted {
                    self.setup.locally_muted.insert(user);
                } else {
                    self.setup.locally_muted.remove(&user);
                }
                self.apply_audio();
            }
            VoiceCmd::StartScreen => {
                if self.screen.is_some() || self.screen_pending {
                    return;
                }
                self.screen_pending = true;
                let (local, tx) = (self.room.local_participant(), self.screen_tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(ScreenShare::start(local).await);
                });
            }
            VoiceCmd::StopScreen => self.stop_screen().await,
            VoiceCmd::Leave => {}
        }
    }

    async fn stop_screen(&mut self) {
        if let Some((share, sid)) = self.screen.take() {
            share.stop();
            let _ = self.room.local_participant().unpublish_track(&sid).await;
        }
    }

    /// Atualiza a interface e o estado de mídia enviado ao servidor.
    fn publish(&mut self) {
        // A captura pode ter terminado sozinha (portal cancelado, janela fechada).
        if self.screen.as_ref().is_some_and(|(share, _)| share.finished()) {
            if let Some((_, sid)) = self.screen.take() {
                let local = self.room.local_participant();
                tokio::spawn(async move {
                    let _ = local.unpublish_track(&sid).await;
                });
            }
        }
        let local = self.room.local_participant();
        let mut participants = vec![ParticipantView {
            identity: local.identity().to_string(),
            name: local.name(),
            local: true,
            speaking: local.is_speaking(),
            muted: self.muted,
            locally_muted: false,
        }];
        for p in self.room.remote_participants().into_values() {
            let identity = p.identity().to_string();
            let muted = p
                .track_publications()
                .values()
                .find(|publication| publication.source() == TrackSource::Microphone)
                .is_none_or(|publication| publication.is_muted());
            participants.push(ParticipantView {
                locally_muted: self.setup.locally_muted.contains(user_of(&identity)),
                identity,
                name: p.name(),
                local: false,
                speaking: p.is_speaking(),
                muted,
            });
        }
        participants[1..].sort_by(|a, b| a.name.cmp(&b.name));
        let mut tiles: Vec<TileView> = self.videos.values().map(|(_, tile)| tile.clone()).collect();
        tiles.sort_by(|a, b| (b.screen, &a.name).cmp(&(a.screen, &b.name)));
        self.setup.ui.send(Event::Voice(Some(VoiceView {
            channel: self.setup.channel.clone(),
            status: self.status,
            muted: self.muted,
            deafened: self.setup.deafened,
            screen: self.screen.is_some(),
            participants,
            tiles,
        })));
        let presence = MediaEvent {
            v: 1,
            kind: "media",
            lease: &self.lease,
            muted: self.muted,
            camera: false,
            screen: self.screen.is_some(),
        };
        self.setup.presence.set(serde_json::to_string(&presence).ok());
    }

    async fn close(mut self) {
        self.stop_screen().await;
        for (_, (task, _)) in self.videos.drain() {
            task.abort();
        }
        self.setup.frames.lock().unwrap().clear();
        let _ = self.room.close().await;
        let _ = self.setup.api.leave(&self.lease).await;
        self.setup.presence.set(None);
        self.setup.ui.send(Event::Voice(None));
    }
}

/// Converte cada frame recebido para RGBA e o deixa disponível para a interface.
async fn receive_video(track: RemoteVideoTrack, key: String, frames: Frames, ui: UiSink) {
    let mut stream = NativeVideoStream::new(track.rtc_track());
    let mut seq = 0u64;
    let mut spare: Vec<u8> = Vec::new();
    while let Some(frame) = stream.next().await {
        let i420 = frame.buffer.to_i420();
        let (width, height) = (i420.width() as usize, i420.height() as usize);
        let (stride_y, stride_u, stride_v) = i420.strides();
        let (y, u, v) = i420.data();
        spare.resize(width * height * 4, 0);
        // "ABGR" do libyuv é R,G,B,A na memória (little-endian): o formato do egui.
        yuv_helper::i420_to_abgr(y, stride_y, u, stride_u, v, stride_v, &mut spare, (width * 4) as u32, width as i32, height as i32);
        seq += 1;
        let data = std::mem::take(&mut spare);
        let previous = frames.lock().unwrap().insert(key.clone(), RgbaFrame { width, height, data, seq });
        // Reaproveita o buffer anterior para evitar uma alocação por frame.
        if let Some(previous) = previous {
            spare = previous.data;
        }
        ui.repaint();
    }
}
