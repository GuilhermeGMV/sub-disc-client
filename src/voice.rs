//! Sessão de voz: entra na sala LiveKit com o token do servidor, publica o
//! microfone pelo áudio nativo do WebRTC (que também toca o áudio recebido),
//! recebe vídeos/telas e compartilha a tela.
use crate::api::Api;
use crate::audio::{AudioKind, Playback, Volume, Volumes, user_of};
use crate::bridge::Device;
use crate::bridge::{
    Event, Frames, ParticipantView, RgbaFrame, TileView, UiSink, VoiceStatus, VoiceView,
};
use crate::camera::CameraShare;
use crate::capture::ShareRequest;
use crate::model::MediaEvent;
use crate::screen::ScreenShare;
use crate::ws::PresenceOut;
use futures_util::StreamExt;
use livekit::PlatformAudio;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub enum VoiceCmd {
    ReapplyAudio,
    SetMuted(bool),
    SetDeafened(bool),
    SetVolume {
        user: String,
        kind: AudioKind,
        volume: Volume,
    },
    StartScreen(ShareRequest),
    StopScreen,
    StartCamera(Device),
    StopCamera,
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
    pub volumes: Volumes,
}

pub fn spawn(setup: VoiceSetup) -> VoiceHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(setup, rx));
    VoiceHandle { tx, task }
}

struct Session {
    setup: VoiceSetup,
    room: Room,
    lease: String,
    mic: LocalAudioTrack,
    status: VoiceStatus,
    muted: bool,
    screen: Option<ScreenShare>,
    camera: Option<CameraShare>,
    camera_pending: bool,
    camera_generation: u64,
    camera_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    camera_tx: mpsc::UnboundedSender<(u64, Result<CameraShare, String>)>,
    /// Seletor de tela aberto; o resultado chega por `screen_rx` sem travar o laço.
    screen_pending: bool,
    screen_jobs: tokio::task::JoinSet<()>,
    screen_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    audios: Playback<RemoteAudioTrack>,
    screen_tx: mpsc::UnboundedSender<(u64, ScreenResult)>,
    screen_generation: u64,
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
        screen_pending: false,
        camera: false,
        camera_pending: false,
        share_video: false,
        share_audio: false,
        participants: vec![],
        tiles: vec![],
    })));
    let (screen_tx, mut screen_rx) = mpsc::unbounded_channel();
    let (camera_tx, mut camera_rx) = mpsc::unbounded_channel();
    let (mut session, mut events) = match connect(setup, screen_tx, camera_tx).await {
        Ok(v) => v,
        Err(e) => {
            ui.error(e);
            ui.send(Event::Voice(None));
            return;
        }
    };
    session.publish();
    // Detecta capturas de tela encerradas fora do app (seletor cancelado etc.).
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                session.apply_audio();
                if !session.screen.as_ref().is_some_and(|share| share.finished()) && !session.camera.as_ref().is_some_and(|share| share.finished()) {
                    continue;
                }
            }
            Some(_) = session.screen_jobs.join_next(), if !session.screen_jobs.is_empty() => continue,
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    log::info!("sala encerrada: {reason:?}");
                    ui.error("A chamada foi encerrada. Você pode entrar novamente.");
                    break;
                }
                Some(event) => session.handle(event),
                None => break,
            },
            Some((generation, result)) = screen_rx.recv() => {
                if generation != session.screen_generation { continue; }
                session.screen_cancel.take();
                session.screen_pending = false;
                match result {
                    Ok(share) => session.screen = Some(share),
                    Err(e) => session.setup.ui.error(e),
                }
            }
            Some((generation, result)) = camera_rx.recv() => {
                if generation != session.camera_generation { continue; }
                session.camera_cancel.take();
                session.camera_pending = false;
                match result { Ok(camera) => session.camera = Some(camera), Err(e) => session.setup.ui.error(e) }
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

type ScreenResult = Result<ScreenShare, String>;

async fn connect(
    setup: VoiceSetup,
    screen_tx: mpsc::UnboundedSender<(u64, ScreenResult)>,
    camera_tx: mpsc::UnboundedSender<(u64, Result<CameraShare, String>)>,
) -> Result<(Session, mpsc::UnboundedReceiver<RoomEvent>), String> {
    let join = setup
        .api
        .join(&setup.channel)
        .await
        .map_err(|e| e.to_string())?;
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
        audio_encoding: Some(AudioEncoding {
            max_bitrate: 96_000,
        }),
        dtx: true,
        red: true,
        ..Default::default()
    };
    if let Err(e) = room
        .local_participant()
        .publish_track(LocalTrack::Audio(mic.clone()), options)
        .await
    {
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
        camera: None,
        camera_pending: false,
        camera_generation: 0,
        camera_cancel: None,
        camera_tx,
        screen_pending: false,
        screen_jobs: tokio::task::JoinSet::new(),
        screen_cancel: None,
        screen_generation: 0,
        audios: Playback::new(),
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
            RoomEvent::TrackSubscribed {
                track, participant, ..
            } => self.attach(track, &participant),
            RoomEvent::TrackUnsubscribed { track, .. } => self.detach(&track.sid()),
            RoomEvent::Reconnecting => self.status = VoiceStatus::Reconnecting,
            RoomEvent::Reconnected => {
                self.status = VoiceStatus::Connected;
                self.audios
                    .update(&self.setup.volumes, self.setup.deafened, true);
            }
            _ => {}
        }
    }

    fn attach(&mut self, track: RemoteTrack, participant: &RemoteParticipant) {
        match track {
            RemoteTrack::Audio(audio) => {
                let kind = if audio.source() == TrackSource::ScreenshareAudio {
                    AudioKind::ScreenShare
                } else {
                    AudioKind::Microphone
                };
                let user = user_of(participant.identity().as_str()).to_owned();
                // Only native ADM playout. No PCM sink or second output is created.
                if !self.audios.attach(
                    audio.sid().to_string(),
                    audio,
                    user,
                    kind,
                    &self.setup.volumes,
                    self.setup.deafened,
                ) {
                    self.setup
                        .ui
                        .error("Não foi possível aplicar o volume desta faixa remota.");
                }
            }
            RemoteTrack::Video(video) => {
                let key = video.sid().to_string();
                let tile = TileView {
                    key: key.clone(),
                    user: user_of(participant.identity().as_str()).to_owned(),
                    name: participant.name(),
                    screen: video.source() == TrackSource::Screenshare,
                };
                let task = tokio::spawn(receive_video(
                    video,
                    key.clone(),
                    self.setup.frames.clone(),
                    self.setup.ui.clone(),
                ));
                if let Some((old, _)) = self.videos.insert(key, (task, tile)) {
                    old.abort();
                }
            }
        }
    }

    fn detach(&mut self, sid: &TrackSid) {
        let key = sid.to_string();
        self.audios.detach(&key);
        if let Some((task, _)) = self.videos.remove(&key) {
            task.abort();
        }
        self.setup.frames.lock().unwrap().remove(&key);
    }

    fn apply_audio(&mut self) {
        self.audios
            .update(&self.setup.volumes, self.setup.deafened, false);
    }

    async fn command(&mut self, cmd: VoiceCmd) {
        match cmd {
            VoiceCmd::ReapplyAudio => {
                self.audios
                    .update(&self.setup.volumes, self.setup.deafened, true)
            }
            VoiceCmd::SetMuted(muted) => {
                self.muted = muted;
                if muted {
                    self.mic.mute()
                } else {
                    self.mic.unmute()
                }
            }
            VoiceCmd::SetDeafened(deafened) => {
                self.setup.deafened = deafened;
                self.apply_audio();
            }
            VoiceCmd::SetVolume { user, kind, volume } => {
                self.setup
                    .volumes
                    .entry(user)
                    .or_default()
                    .set(kind, volume);
            }
            VoiceCmd::StartScreen(request) => {
                if self.screen_pending {
                    return;
                }
                if let Err(error) = request.validate(false) {
                    self.setup.ui.error(error);
                    return;
                }
                self.stop_screen().await;
                self.screen_pending = true;
                let generation = self.screen_generation;
                let (local, tx) = (self.room.local_participant(), self.screen_tx.clone());
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
                self.screen_cancel = Some(cancel_tx);
                self.screen_jobs.spawn(async move {
                    let result = ScreenShare::start(local, request, cancel_rx).await;
                    let _ = tx.send((generation, result));
                });
            }
            VoiceCmd::StartCamera(device) => {
                if self.camera_pending {
                    return;
                }
                self.stop_camera().await;
                self.camera_pending = true;
                let generation = self.camera_generation;
                let (local, tx) = (self.room.local_participant(), self.camera_tx.clone());
                let (frames, ui) = (self.setup.frames.clone(), self.setup.ui.clone());
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
                self.camera_cancel = Some(cancel_tx);
                self.screen_jobs.spawn(async move {
                    let result = CameraShare::start(local, device, frames, ui, cancel_rx).await;
                    let _ = tx.send((generation, result));
                });
            }
            VoiceCmd::StopCamera => self.stop_camera().await,
            VoiceCmd::StopScreen => self.stop_screen().await,
            VoiceCmd::Leave => {}
        }
    }

    async fn stop_camera(&mut self) {
        self.camera_generation = self.camera_generation.wrapping_add(1);
        if let Some(cancel) = self.camera_cancel.take() {
            let _ = cancel.send(());
        }
        self.camera_pending = false;
        if let Some(camera) = self.camera.take() {
            camera.close().await;
        }
    }

    async fn stop_screen(&mut self) {
        self.screen_generation = self.screen_generation.wrapping_add(1);
        if let Some(cancel) = self.screen_cancel.take() {
            let _ = cancel.send(());
        }
        self.screen_pending = false;
        if let Some(share) = self.screen.take() {
            share.close().await;
        }
    }

    /// Atualiza a interface e o estado de mídia enviado ao servidor.
    fn publish(&mut self) {
        // A captura pode ter terminado sozinha (portal cancelado, janela fechada).
        if self.screen.as_ref().is_some_and(|share| share.finished()) {
            self.screen.take();
            self.setup
                .ui
                .error("A captura foi encerrada ou a fonte ficou indisponível.");
        }
        if self.camera.as_ref().is_some_and(|camera| camera.finished()) {
            self.camera.take();
            self.setup
                .ui
                .error("A câmera foi desconectada ou parou de enviar vídeo.");
        }
        let local = self.room.local_participant();
        let mut participants = vec![ParticipantView {
            identity: local.identity().to_string(),
            name: local.name(),
            local: true,
            speaking: local.is_speaking(),
            muted: self.muted,
            volume: Default::default(),
            screen_audio: false,
        }];
        for p in self.room.remote_participants().into_values() {
            let identity = p.identity().to_string();
            let muted = p
                .track_publications()
                .values()
                .find(|publication| publication.source() == TrackSource::Microphone)
                .is_none_or(|publication| publication.is_muted());
            participants.push(ParticipantView {
                volume: self
                    .setup
                    .volumes
                    .get(user_of(&identity))
                    .copied()
                    .unwrap_or_default(),
                screen_audio: p
                    .track_publications()
                    .values()
                    .any(|p| p.source() == TrackSource::ScreenshareAudio),
                identity,
                name: p.name(),
                local: false,
                speaking: p.is_speaking(),
                muted,
            });
        }
        participants[1..].sort_by(|a, b| a.name.cmp(&b.name));
        let mut tiles: Vec<TileView> = self.videos.values().map(|(_, tile)| tile.clone()).collect();
        if let Some(camera) = &self.camera {
            tiles.push(TileView {
                key: camera.key.clone(),
                name: "Você · câmera".into(),
                screen: false,
                user: user_of(&local.identity().to_string()).into(),
            });
        }
        tiles.sort_by(|a, b| (b.screen, &a.name).cmp(&(a.screen, &b.name)));
        self.setup.ui.send(Event::Voice(Some(VoiceView {
            channel: self.setup.channel.clone(),
            status: self.status,
            muted: self.muted,
            deafened: self.setup.deafened,
            screen: self.screen.is_some(),
            screen_pending: self.screen_pending,
            camera: self.camera.is_some(),
            camera_pending: self.camera_pending,
            share_video: self.screen.is_some(),
            share_audio: false,
            participants,
            tiles,
        })));
        let presence = MediaEvent {
            v: 1,
            kind: "media",
            lease: &self.lease,
            muted: self.muted,
            camera: self.camera.is_some(),
            screen: self.screen.is_some(),
        };
        self.setup
            .presence
            .set(serde_json::to_string(&presence).ok());
    }

    async fn close(mut self) {
        self.stop_camera().await;
        self.stop_screen().await;
        for (_, (task, _)) in self.videos.drain() {
            task.abort();
        }
        self.audios.clear();
        self.setup.frames.lock().unwrap().clear();
        let _ = self.room.close().await;
        // Publications already in flight finish and unpublish on cancellation.
        // Aborting is safe only after the room has closed all its tracks.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while self.screen_jobs.join_next().await.is_some() {}
        })
        .await;
        self.screen_jobs.shutdown().await;
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
        yuv_helper::i420_to_abgr(
            y,
            stride_y,
            u,
            stride_u,
            v,
            stride_v,
            &mut spare,
            (width * 4) as u32,
            width as i32,
            height as i32,
        );
        seq += 1;
        let data = std::mem::take(&mut spare);
        let previous = frames.lock().unwrap().insert(
            key.clone(),
            RgbaFrame {
                width,
                height,
                data,
                seq,
            },
        );
        // Reaproveita o buffer anterior para evitar uma alocação por frame.
        if let Some(previous) = previous {
            spare = previous.data;
        }
        ui.repaint();
    }
}
