//! Backend inteiramente em memória. Não importa Api, Config, WebSocket,
//! PlatformAudio, Room ou ScreenShare e nunca enumera dispositivos/fontes reais.
use crate::audio::{Volumes, user_of};
use crate::bridge::{
    Command, Device, Event, Frames, ParticipantView, RgbaFrame, TileView, UiSink, VoiceStatus,
    VoiceView,
};
use crate::capture::{CaptureKind, CaptureSourceInfo};
use crate::model::{Channel, ChannelKind, ChatMessage, Snapshot, User, VoicePresence};
use std::collections::HashMap;
use tokio::sync::mpsc;

struct Demo {
    snapshot: Snapshot,
    voice: Option<VoiceView>,
    volumes: Volumes,
    messages: HashMap<String, Vec<ChatMessage>>,
    next_id: i64,
    seq: u64,
}

fn sources() -> Vec<CaptureSourceInfo> {
    vec![
        CaptureSourceInfo {
            id: 1,
            title: "Monitor 1 (simulado)".into(),
            kind: CaptureKind::Monitor,
        },
        CaptureSourceInfo {
            id: 2,
            title: "Monitor 2 (simulado)".into(),
            kind: CaptureKind::Monitor,
        },
        CaptureSourceInfo {
            id: 3,
            title: "Editor — Projeto Subdisc (simulado)".into(),
            kind: CaptureKind::Window,
        },
        CaptureSourceInfo {
            id: 4,
            title: "Firefox — Documentação (janela simulada)".into(),
            kind: CaptureKind::Window,
        },
    ]
}

impl Demo {
    fn new() -> Self {
        let users: Vec<User> = [
            "Desenvolvedor",
            "Guilherme",
            "Lucas",
            "Ana",
            "Marina",
            "Pedro",
            "Beatriz",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| User {
            role: if i == 0 { "admin" } else { "member" }.into(),
            id: format!("demo-{i}"),
            username: (*name).into(),
        })
        .collect();
        let channels = [
            ("geral", ChannelKind::Text),
            ("projetos", ChannelKind::Text),
            ("Conversa", ChannelKind::Voice),
            ("Jogos", ChannelKind::Voice),
            ("Estúdio", ChannelKind::Voice),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, kind))| Channel {
            id: format!("c{i}"),
            name: name.into(),
            kind,
        })
        .collect();
        let snapshot = Snapshot {
            online: users[..6].iter().map(|u| u.id.clone()).collect(),
            voice: users[..5]
                .iter()
                .enumerate()
                .map(|(i, u)| VoicePresence {
                    user_id: u.id.clone(),
                    channel_id: "c2".into(),
                    connected: true,
                    muted: i == 3,
                    screen: i == 1 || i == 2,
                })
                .collect(),
            users,
            channels,
        };
        let mut demo = Self {
            snapshot,
            voice: None,
            volumes: Volumes::new(),
            messages: HashMap::new(),
            next_id: 1,
            seq: 0,
        };
        for channel in ["c0", "c1"] {
            for (name, text) in [
                (
                    "Guilherme",
                    "Bem-vindo ao Subdisc! Este ambiente funciona totalmente offline.",
                ),
                (
                    "Ana",
                    "Podemos testar os volumes da voz e do compartilhamento separadamente.",
                ),
                (
                    "Lucas",
                    "Estou compartilhando uma apresentação. Clique no vídeo para ampliar.",
                ),
                (
                    "Marina",
                    "O seletor de fontes está no botão Tela. Todos os controles aqui são simulações.",
                ),
            ] {
                demo.message(channel.into(), name, text.into());
            }
        }
        demo.join("c2".into());
        demo
    }

    fn message(&mut self, channel: String, name: &str, content: String) -> ChatMessage {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let message = ChatMessage {
            id: self.next_id,
            channel_id: channel.clone(),
            username: name.into(),
            content,
            created_at: now - 600 + self.next_id * 30,
        };
        self.next_id += 1;
        self.messages
            .entry(channel)
            .or_default()
            .push(message.clone());
        message
    }

    fn join(&mut self, channel: String) {
        let participants = self.snapshot.users[..5]
            .iter()
            .enumerate()
            .map(|(i, u)| ParticipantView {
                identity: format!("{}:simulado", u.id),
                name: u.username.clone(),
                local: i == 0,
                speaking: i == 1,
                muted: i == 3,
                volume: self.volumes.get(&u.id).copied().unwrap_or_default(),
                screen_audio: i == 1 || i == 2,
            })
            .collect();
        let tiles = [
            ("demo-screen-1", 1, true),
            ("demo-screen-2", 2, true),
            ("demo-camera", 4, false),
        ]
        .into_iter()
        .map(|(key, i, screen)| TileView {
            key: key.into(),
            name: format!("{} · simulação", self.snapshot.users[i].username),
            screen,
            user: self.snapshot.users[i].id.clone(),
        })
        .collect();
        self.snapshot.voice = self.snapshot.users[..5]
            .iter()
            .enumerate()
            .map(|(i, u)| VoicePresence {
                user_id: u.id.clone(),
                channel_id: channel.clone(),
                connected: true,
                muted: i == 3,
                screen: i == 1 || i == 2,
            })
            .collect();
        self.voice = Some(VoiceView {
            channel,
            status: VoiceStatus::Connected,
            muted: false,
            deafened: false,
            screen: false,
            screen_pending: false,
            camera: false,
            camera_pending: false,
            share_video: false,
            share_audio: false,
            participants,
            tiles,
        });
    }

    fn command(&mut self, command: Command) -> Vec<Event> {
        let mut events = vec![];
        match command {
            Command::LoadMessages { channel, before } => {
                let messages = self
                    .messages
                    .get(&channel)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|m| before.is_none_or(|id| m.id < id))
                    .collect();
                events.push(Event::Messages {
                    channel,
                    messages,
                    older: before.is_some(),
                });
            }
            Command::SendMessage { channel, content } => {
                if !content.trim().is_empty() {
                    events.push(Event::Message(self.message(
                        channel,
                        "Desenvolvedor",
                        content,
                    )));
                }
            }
            Command::JoinVoice { channel } => self.join(channel),
            Command::LeaveVoice => {
                self.voice = None;
                self.snapshot.voice.retain(|p| p.user_id != "demo-0");
            }
            Command::ListCameras => events.push(Event::Cameras(Ok(vec![crate::bridge::Device {
                id: "demo-camera".into(),
                name: "Câmera simulada".into(),
            }]))),
            Command::StartCamera(_) => {
                if let Some(v) = &mut self.voice {
                    v.camera = true;
                    v.tiles.retain(|t| t.key != "demo-local-camera");
                    v.tiles.push(TileView {
                        key: "demo-local-camera".into(),
                        name: "Você · câmera".into(),
                        screen: false,
                        user: "demo-0".into(),
                    });
                }
            }
            Command::StopCamera => {
                if let Some(v) = &mut self.voice {
                    v.camera = false;
                    v.tiles.retain(|t| t.key != "demo-local-camera");
                }
            }
            Command::SetMuted(muted) => {
                if let Some(v) = &mut self.voice {
                    v.muted = muted;
                    v.participants[0].muted = muted;
                }
            }
            Command::SetDeafened(deafened) => {
                if let Some(v) = &mut self.voice {
                    v.deafened = deafened;
                }
            }
            Command::SetVolume { user, kind, volume } => {
                let user = user_of(&user);
                let prefs = self.volumes.entry(user.into()).or_default();
                prefs.set(kind, volume);
                if let Some(v) = &mut self.voice {
                    for p in &mut v.participants {
                        if user_of(&p.identity) == user {
                            p.volume = *prefs;
                        }
                    }
                }
            }
            Command::ListSources { request } => events.push(Event::Sources {
                request,
                result: Ok(sources()),
            }),
            Command::StartScreen(request) => {
                if let Err(error) = request.validate(true).and_then(|()| {
                    crate::capture::selected(&sources(), &request.source).map(|_| ())
                }) {
                    events.push(Event::Error(error));
                } else if let Some(v) = &mut self.voice {
                    v.screen = true;
                    v.share_video = request.video;
                    v.share_audio = request.audio;
                    v.tiles.retain(|t| t.key != "demo-local");
                    v.participants[0].screen_audio = request.audio;
                    if request.video {
                        v.tiles.push(TileView {
                            key: "demo-local".into(),
                            name: format!("{} · simulação", request.source.title),
                            screen: true,
                            user: "demo-0".into(),
                        });
                    }
                    events.push(Event::Error(format!(
                        "SIMULAÇÃO: transmissão iniciada (vídeo: {}, áudio: {}).",
                        request.video, request.audio
                    )));
                }
            }
            Command::StopScreen => {
                if let Some(v) = &mut self.voice {
                    v.screen = false;
                    v.share_video = false;
                    v.share_audio = false;
                    v.participants[0].screen_audio = false;
                    v.tiles.retain(|t| t.key != "demo-local");
                }
            }
            Command::ListDevices => events.push(Event::Devices {
                mics: vec![Device {
                    id: "fake-mic".into(),
                    name: "Microfone simulado".into(),
                }],
                speakers: vec![Device {
                    id: "fake-out".into(),
                    name: "Fones simulados".into(),
                }],
            }),
            Command::DemoState(status) => {
                if let Some(v) = &mut self.voice {
                    v.status = status;
                }
                events.push(Event::Connection(status == VoiceStatus::Connected));
            }
            Command::DemoError => events.push(Event::Error(
                "SIMULAÇÃO: conexão interrompida; use Reconectar para testar a recuperação.".into(),
            )),
            Command::Logout => events.push(Event::Error(
                "SIMULAÇÃO: nenhuma sessão real está aberta. Feche a janela para sair do demo."
                    .into(),
            )),
            Command::SelectMic(_) | Command::SelectSpeaker(_) | Command::SetProcessing { .. } => {
                events.push(Event::Error(
                    "SIMULAÇÃO: configuração aplicada apenas em memória.".into(),
                ));
            }
            Command::Admin { .. } => events.push(Event::AdminFinished(Err(
                "Administração indisponível no demo offline.".into(),
            ))),
            Command::Probe { .. } | Command::Login { .. } => {}
        }
        if let Some(v) = &self.voice
            && let Some(p) = self
                .snapshot
                .voice
                .iter_mut()
                .find(|p| p.user_id == "demo-0")
        {
            p.screen = v.screen;
            p.muted = v.muted;
        }
        events.push(Event::Presence {
            online: self.snapshot.online.clone(),
            voice: self.snapshot.voice.clone(),
        });
        events.push(Event::Voice(self.voice.clone()));
        events
    }

    fn animate(&mut self, frames: &Frames) {
        self.seq += 1;
        let mut frames = frames.lock().unwrap();
        let Some(voice) = &mut self.voice else {
            frames.clear();
            return;
        };
        for (i, p) in voice.participants.iter_mut().enumerate() {
            p.speaking = !p.muted && (self.seq / 6) as usize % 5 == i;
        }
        frames.retain(|key, _| voice.tiles.iter().any(|t| &t.key == key));
        for (i, tile) in voice.tiles.iter().enumerate() {
            let (width, height) = (320, 180);
            let frame = frames.entry(tile.key.clone()).or_insert_with(|| RgbaFrame {
                width,
                height,
                data: vec![0; width * height * 4],
                seq: 0,
            });
            for (index, pixel) in frame.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x, y) = (index % width, index / width);
                let pattern = (((x + self.seq as usize * 2) / 24 + y / 24) % 2) as u8;
                pixel.copy_from_slice(&[
                    25 + i as u8 * 22,
                    45 + pattern * 35,
                    60 + (x * 70 / width) as u8,
                    255,
                ]);
            }
            frame.seq = self.seq;
        }
    }
}

pub async fn run(mut commands: mpsc::UnboundedReceiver<Command>, ui: UiSink, frames: Frames) {
    let mut demo = Demo::new();
    ui.send(Event::LoggedIn {
        user: demo.snapshot.users[0].clone(),
    });
    ui.send(Event::Connection(true));
    ui.send(Event::Snapshot(demo.snapshot.clone()));
    ui.send(Event::Voice(demo.voice.clone()));
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => for event in demo.command(command) { ui.send(event); },
                None => break,
            },
            _ = tick.tick() => {
                demo.animate(&frames);
                ui.send(Event::Voice(demo.voice.clone()));
            }
        }
    }
    frames.lock().unwrap().clear();
}

#[cfg(test)]
pub(crate) fn ui_fixture() -> (Vec<Event>, Frames) {
    let mut demo = Demo::new();
    let frames = Frames::default();
    demo.animate(&frames);
    let mut events = vec![
        Event::LoggedIn {
            user: demo.snapshot.users[0].clone(),
        },
        Event::Snapshot(demo.snapshot.clone()),
        Event::Voice(demo.voice.clone()),
    ];
    events.extend(demo.command(Command::LoadMessages {
        channel: "c0".into(),
        before: None,
    }));
    (events, frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioKind, Volume};

    #[test]
    fn offline_scenario_and_interactions() {
        let mut demo = Demo::new();
        assert!(demo.snapshot.users.len() >= 6);
        assert_eq!(
            demo.snapshot
                .channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Voice)
                .count(),
            3
        );
        assert_eq!(demo.voice.as_ref().unwrap().tiles.len(), 3);
        assert!(demo.snapshot.online.len() < demo.snapshot.users.len());
        let events = demo.command(Command::SendMessage {
            channel: "c0".into(),
            content: "Teste".into(),
        });
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Message(m) if m.content == "Teste"))
        );
        demo.command(Command::SetVolume {
            user: "demo-1".into(),
            kind: AudioKind::ScreenShare,
            volume: Volume {
                gain: 0.35,
                muted: true,
            },
        });
        demo.command(Command::LeaveVoice);
        assert!(demo.voice.is_none());
        demo.command(Command::JoinVoice {
            channel: "c3".into(),
        });
        let p = &demo.voice.as_ref().unwrap().participants[1];
        assert_eq!(p.volume.screen_share.gain, 0.35);
        assert!(p.volume.screen_share.muted);
        assert_eq!(p.volume.microphone.gain, 1.0);
        let frames = Frames::default();
        demo.animate(&frames);
        assert_eq!(frames.lock().unwrap().len(), 3);
        demo.command(Command::LeaveVoice);
        demo.animate(&frames);
        assert!(frames.lock().unwrap().is_empty());
    }

    #[test]
    fn simulated_capture_modes_and_cleanup() {
        let mut demo = Demo::new();
        for (video, audio) in [(true, true), (true, false), (false, true), (false, false)] {
            demo.command(Command::StartScreen(crate::capture::ShareRequest {
                source: sources()[0].clone(),
                video,
                audio,
            }));
            let v = demo.voice.as_ref().unwrap();
            assert_eq!(v.screen, video || audio);
            assert_eq!(v.tiles.iter().any(|t| t.key == "demo-local"), video);
            demo.command(Command::StopScreen);
            assert!(!demo.voice.as_ref().unwrap().screen);
        }
    }

    #[test]
    fn camera_and_screen_are_independent_and_leave_clears_preview() {
        let mut demo = Demo::new();
        let frames = Frames::default();
        let devices = demo.command(Command::ListCameras);
        let device = devices
            .into_iter()
            .find_map(|e| match e {
                Event::Cameras(Ok(devices)) => devices.into_iter().next(),
                _ => None,
            })
            .unwrap();
        demo.command(Command::StartScreen(crate::capture::ShareRequest {
            source: sources()[0].clone(),
            video: true,
            audio: false,
        }));
        demo.command(Command::StartCamera(device.clone()));
        demo.animate(&frames);
        assert!(demo.voice.as_ref().unwrap().camera);
        assert!(frames.lock().unwrap().contains_key("demo-local-camera"));
        demo.command(Command::StopCamera);
        demo.animate(&frames);
        assert!(!demo.voice.as_ref().unwrap().camera);
        assert!(demo.voice.as_ref().unwrap().screen);
        assert!(!frames.lock().unwrap().contains_key("demo-local-camera"));
        demo.command(Command::StartCamera(device));
        demo.command(Command::LeaveVoice);
        demo.animate(&frames);
        assert!(frames.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn backend_starts_and_stops_without_server_or_devices() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, received) = std::sync::mpsc::channel();
        let frames = Frames::default();
        let task = tokio::spawn(run(
            rx,
            UiSink::new(events, Default::default()),
            frames.clone(),
        ));
        tx.send(Command::ListDevices).unwrap();
        tx.send(Command::ListSources { request: 1 }).unwrap();
        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let events: Vec<_> = received.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::LoggedIn { user } if user.username == "Desenvolvedor"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Sources { result: Ok(s), .. } if s.len() == 4))
        );
        assert!(frames.lock().unwrap().is_empty());
    }
}
