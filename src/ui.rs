//! Interface egui: entrada, canais, chat e chamada com vídeos.
use crate::audio::{AudioKind, Volume, user_of};
use crate::bridge::{Command, Device, Event, Frames, LoginMode, VoiceStatus, VoiceView};
use crate::config::Config;
use crate::model::{ChannelKind, ChatMessage, Snapshot, User};
use crate::ui_capture::ShareDialog;
use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions, Vec2};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const ACCENT: Color32 = Color32::from_rgb(0x9e, 0xd7, 0xb0);
const ERROR_TTL: Duration = Duration::from_secs(8);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Login,
    Register,
    Setup,
}

enum Screen {
    Loading,
    Login,
    Main,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Chat,
    Call,
}

struct LoginForm {
    server: String,
    username: String,
    password: String,
    extra: String,
    tab: Tab,
    needs_setup: bool,
    busy: bool,
    probed: String,
}

struct AudioSettings {
    open: bool,
    mics: Vec<Device>,
    speakers: Vec<Device>,
    mic: String,
    speaker: String,
    echo: bool,
    agc: bool,
    noise: bool,
}

pub struct App {
    cmd: UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    frames: Frames,
    textures: HashMap<String, (TextureHandle, u64)>,
    screen: Screen,
    form: LoginForm,
    error: Option<(String, Instant)>,
    user: Option<User>,
    snapshot: Snapshot,
    connected: bool,
    selected: Option<String>,
    messages: HashMap<String, Vec<ChatMessage>>,
    has_more: HashMap<String, bool>,
    draft: String,
    voice: Option<VoiceView>,
    view: View,
    expanded: Option<String>,
    fullscreen: bool,
    audio: AudioSettings,
    demo: bool,
    share: ShareDialog,
}

impl App {
    pub fn new(
        cmd: UnboundedSender<Command>,
        events: mpsc::Receiver<Event>,
        frames: Frames,
        demo: bool,
    ) -> Self {
        let config = if demo {
            Config::default()
        } else {
            Config::load()
        };
        Self {
            demo,
            share: ShareDialog::default(),
            cmd,
            events,
            frames,
            textures: HashMap::new(),
            screen: Screen::Loading,
            form: LoginForm {
                server: config.server,
                username: config.username,
                password: String::new(),
                extra: String::new(),
                tab: Tab::Login,
                needs_setup: false,
                busy: false,
                probed: String::new(),
            },
            error: None,
            user: None,
            snapshot: Snapshot::default(),
            connected: false,
            selected: None,
            messages: HashMap::new(),
            has_more: HashMap::new(),
            draft: String::new(),
            voice: None,
            view: View::Chat,
            expanded: None,
            fullscreen: false,
            audio: AudioSettings {
                open: false,
                mics: vec![],
                speakers: vec![],
                mic: String::new(),
                speaker: String::new(),
                echo: true,
                agc: true,
                noise: true,
            },
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmd.send(cmd);
    }

    fn show_error(&mut self, message: String) {
        self.error = Some((message, Instant::now()));
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Sources { request, result } => self.share.receive(request, result),
            Event::Probed { needs_setup } => {
                self.form.needs_setup = needs_setup;
                if needs_setup {
                    self.form.tab = Tab::Setup;
                } else if self.form.tab == Tab::Setup {
                    self.form.tab = Tab::Login;
                }
            }
            Event::LoggedIn { user } => {
                self.form.password.clear();
                self.form.extra.clear();
                self.form.busy = false;
                self.user = Some(user);
                self.screen = Screen::Main;
                self.error = None;
            }
            Event::LoggedOut { error } => {
                self.form.busy = false;
                self.user = None;
                self.snapshot = Snapshot::default();
                self.messages.clear();
                self.selected = None;
                self.voice = None;
                self.screen = Screen::Login;
                if let Some(e) = error {
                    self.show_error(e);
                }
            }
            Event::Connection(connected) => self.connected = connected,
            Event::Snapshot(snapshot) => {
                self.snapshot = snapshot;
                let text = |id: &String| {
                    self.snapshot
                        .channels
                        .iter()
                        .any(|c| &c.id == id && c.kind == ChannelKind::Text)
                };
                if !self.selected.as_ref().is_some_and(text) {
                    self.selected = self
                        .snapshot
                        .channels
                        .iter()
                        .find(|c| c.kind == ChannelKind::Text)
                        .map(|c| c.id.clone());
                }
                // Recarrega o histórico recente após (re)conectar, sem duplicar mensagens.
                if let Some(channel) = self.selected.clone() {
                    self.send(Command::LoadMessages {
                        channel,
                        before: None,
                    });
                }
            }
            Event::Presence { online, voice } => {
                self.snapshot.online = online;
                self.snapshot.voice = voice;
            }
            Event::Message(message) => merge(
                self.messages.entry(message.channel_id.clone()).or_default(),
                vec![message],
            ),
            Event::Messages {
                channel,
                messages,
                older,
            } => {
                if older || !self.has_more.contains_key(&channel) {
                    self.has_more.insert(channel.clone(), messages.len() >= 50);
                }
                merge(self.messages.entry(channel).or_default(), messages);
            }
            Event::Voice(voice) => {
                if voice.is_some() && self.voice.is_none() {
                    self.view = View::Call;
                }
                if voice.is_none() {
                    self.share.open = false;
                    self.view = View::Chat;
                    self.expanded = None;
                }
                self.voice = voice;
            }
            Event::Devices { mics, speakers } => {
                if self.demo {
                    if self.audio.mic.is_empty() {
                        self.audio.mic = mics.first().map_or_else(String::new, |d| d.id.clone());
                    }
                    if self.audio.speaker.is_empty() {
                        self.audio.speaker =
                            speakers.first().map_or_else(String::new, |d| d.id.clone());
                    }
                }
                self.audio.mics = mics;
                self.audio.speakers = speakers;
            }
            Event::Error(message) => self.show_error(message),
        }
    }

    fn login_screen(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.15).max(16.0));
                ui.heading(RichText::new("Subdisc").size(32.0).color(ACCENT));
                ui.label("Seu ponto de encontro.");
                ui.add_space(20.0);
                ui.set_max_width(340.0);
                let form = &mut self.form;
                ui.label("Servidor");
                let server = ui.add(
                    egui::TextEdit::singleline(&mut form.server)
                        .hint_text("chat.exemplo.com")
                        .desired_width(f32::INFINITY),
                );
                if server.lost_focus()
                    && !form.server.trim().is_empty()
                    && form.probed != form.server
                {
                    form.probed = form.server.clone();
                    let _ = self.cmd.send(Command::Probe {
                        server: form.server.clone(),
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut form.tab, Tab::Login, "Entrar");
                    ui.selectable_value(&mut form.tab, Tab::Register, "Tenho um convite");
                    if form.needs_setup {
                        ui.selectable_value(&mut form.tab, Tab::Setup, "Criar administrador");
                    }
                });
                ui.add_space(8.0);
                ui.label("Nome de usuário");
                ui.add(egui::TextEdit::singleline(&mut form.username).desired_width(f32::INFINITY));
                ui.label("Senha");
                let password = ui.add(
                    egui::TextEdit::singleline(&mut form.password)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                match form.tab {
                    Tab::Register => {
                        ui.label("Convite (link ou código)");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.extra)
                                .desired_width(f32::INFINITY),
                        );
                    }
                    Tab::Setup => {
                        ui.label("Chave de instalação (SETUP_KEY)");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.extra)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        );
                    }
                    Tab::Login => {}
                }
                ui.add_space(12.0);
                let label = match form.tab {
                    Tab::Login => "Entrar",
                    Tab::Register => "Criar conta",
                    Tab::Setup => "Criar minha instância",
                };
                let enter = password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui
                    .add_enabled(
                        !form.busy,
                        egui::Button::new(label).min_size(Vec2::new(340.0, 32.0)),
                    )
                    .clicked();
                if (clicked || enter) && !form.busy {
                    let extra = form.extra.trim().to_owned();
                    let mode = match form.tab {
                        Tab::Login => LoginMode::Login,
                        Tab::Register => LoginMode::Register {
                            invite: extra
                                .rsplit("invite=")
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                        },
                        Tab::Setup => LoginMode::Setup { setup_key: extra },
                    };
                    form.busy = true;
                    let _ = self.cmd.send(Command::Login {
                        server: form.server.clone(),
                        username: form.username.trim().to_owned(),
                        password: form.password.clone(),
                        mode,
                    });
                }
                if form.busy {
                    ui.spinner();
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=400.0)
            .show(ui, |ui| {
                egui::Panel::bottom("me").show(ui, |ui| {
                    ui.add_space(6.0);
                    if self.voice.is_some() {
                        self.voice_controls(ui);
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        let name = self
                            .user
                            .as_ref()
                            .map(|u| u.username.clone())
                            .unwrap_or_default();
                        ui.label(RichText::new(name).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Sair").on_hover_text("Sair da conta").clicked() {
                                self.send(Command::Logout);
                            }
                            if ui
                                .button("⚙")
                                .on_hover_text("Dispositivos de áudio")
                                .clicked()
                            {
                                self.audio.open = true;
                                log::debug!("abrindo janela de dispositivos (demo: {})", self.demo);
                                if self.audio.open && self.audio.mics.is_empty() {
                                    self.send(Command::ListDevices);
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Subdisc").color(ACCENT));
                    let (color, text) = if self.connected {
                        (ACCENT, "Conectado")
                    } else {
                        (Color32::from_rgb(0xd9, 0xa3, 0x6b), "Reconectando…")
                    };
                    ui.label(RichText::new("●").color(color))
                        .on_hover_text(text);
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("CONVERSAS").small().weak());
                        let channels = self.snapshot.channels.clone();
                        for channel in channels.iter().filter(|c| c.kind == ChannelKind::Text) {
                            let selected = self.selected.as_ref() == Some(&channel.id)
                                && self.view == View::Chat;
                            if ui
                                .selectable_label(selected, format!("# {}", channel.name))
                                .clicked()
                            {
                                self.view = View::Chat;
                                if self.selected.as_ref() != Some(&channel.id) {
                                    self.selected = Some(channel.id.clone());
                                    self.send(Command::LoadMessages {
                                        channel: channel.id.clone(),
                                        before: None,
                                    });
                                }
                            }
                        }
                        ui.add_space(10.0);
                        ui.label(RichText::new("PESSOAS").small().weak());
                        for user in &self.snapshot.users {
                            let online = self.snapshot.online.contains(&user.id);
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    if online { "●" } else { "○" },
                                    user.username
                                ))
                                .small()
                                .color(if online {
                                    ACCENT
                                } else {
                                    Color32::GRAY
                                }),
                            );
                        }
                        ui.add_space(10.0);
                        ui.label(RichText::new("SALAS DE VOZ").small().weak());
                        for channel in channels.iter().filter(|c| c.kind == ChannelKind::Voice) {
                            let here = self.voice.as_ref().is_some_and(|v| v.channel == channel.id);
                            let response = ui.selectable_label(
                                here && self.view == View::Call,
                                format!("🔊 {}", channel.name),
                            );
                            if response.clicked() {
                                if here {
                                    self.view = View::Call;
                                } else {
                                    self.send(Command::JoinVoice {
                                        channel: channel.id.clone(),
                                    });
                                }
                            }
                            for presence in self
                                .snapshot
                                .voice
                                .iter()
                                .filter(|v| v.channel_id == channel.id && v.connected)
                            {
                                let name = self
                                    .snapshot
                                    .users
                                    .iter()
                                    .find(|u| u.id == presence.user_id)
                                    .map_or("?", |u| u.username.as_str());
                                let mut text = format!("      {name}");
                                if presence.muted {
                                    text.push_str("  🔇");
                                }
                                if presence.screen {
                                    text.push_str("  🖥");
                                }
                                ui.label(RichText::new(text).small());
                            }
                        }
                    });
            });
    }

    fn voice_controls(&mut self, ui: &mut egui::Ui) {
        let Some(voice) = self.voice.clone() else {
            return;
        };
        let name = self
            .snapshot
            .channels
            .iter()
            .find(|c| c.id == voice.channel)
            .map_or("Sala", |c| c.name.as_str());
        let status = match voice.status {
            VoiceStatus::Connecting => "Conectando…",
            VoiceStatus::Connected => "Chamada conectada",
            VoiceStatus::Reconnecting => "Reconectando…",
        };
        ui.label(RichText::new(status).color(ACCENT).small());
        ui.label(RichText::new(name).small());
        ui.horizontal_wrapped(|ui| {
            let mic = if voice.muted {
                "🔇 Ativar"
            } else {
                "🎤 Silenciar"
            };
            if ui
                .selectable_label(voice.muted, mic)
                .on_hover_text("Microfone")
                .clicked()
            {
                self.send(Command::SetMuted(!voice.muted));
            }
            if ui
                .selectable_label(voice.deafened, "🎧")
                .on_hover_text("Silenciar áudio recebido")
                .clicked()
            {
                self.send(Command::SetDeafened(!voice.deafened));
            }
            let screen = if voice.screen_pending {
                "Cancelar captura"
            } else if voice.screen {
                "🖥 Parar"
            } else {
                "🖥 Tela"
            };
            if ui
                .selectable_label(voice.screen, screen)
                .on_hover_text("Compartilhar tela")
                .clicked()
            {
                if voice.screen || voice.screen_pending {
                    self.send(Command::StopScreen);
                } else {
                    self.share.open = true;
                    let command = self.share.refresh();
                    self.send(command);
                }
            }
            if voice.screen && ui.small_button("Trocar fonte").clicked() {
                self.share.open = true;
                let command = self.share.refresh();
                self.send(command);
            }
            if ui
                .button(RichText::new("📞 Sair").color(Color32::from_rgb(0xe0, 0x7a, 0x7a)))
                .clicked()
            {
                self.send(Command::LeaveVoice);
            }
        });
    }

    fn chat(&mut self, ui: &mut egui::Ui) {
        let Some(channel) = self.selected.clone() else {
            ui.centered_and_justified(|ui| ui.label("Escolha uma conversa na lateral."));
            return;
        };
        let name = self
            .snapshot
            .channels
            .iter()
            .find(|c| c.id == channel)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        egui::Panel::top("chat-title").show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading(format!("# {name}"));
            ui.add_space(4.0);
        });
        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.add_space(6.0);
            let input = ui.add(
                egui::TextEdit::singleline(&mut self.draft)
                    .hint_text(format!("Conversar em #{name}"))
                    .char_limit(4000)
                    .desired_width(f32::INFINITY),
            );
            if input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let content = self.draft.trim().to_owned();
                if !content.is_empty() {
                    self.send(Command::SendMessage {
                        channel: channel.clone(),
                        content,
                    });
                    self.draft.clear();
                }
                input.request_focus();
            }
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let messages = self.messages.get(&channel).cloned().unwrap_or_default();
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.has_more.get(&channel).copied().unwrap_or(false)
                        && ui.button("Carregar mensagens anteriores").clicked()
                    {
                        let before = messages.first().map(|m| m.id);
                        self.send(Command::LoadMessages {
                            channel: channel.clone(),
                            before,
                        });
                    }
                    if messages.is_empty() {
                        ui.label(RichText::new("A conversa começa aqui.").weak());
                    }
                    let mut previous: Option<&ChatMessage> = None;
                    for message in &messages {
                        let grouped = previous.is_some_and(|p| {
                            p.username == message.username
                                && message.created_at - p.created_at < 300
                        });
                        if !grouped {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&message.username).strong().color(ACCENT));
                                ui.label(
                                    RichText::new(format_time(message.created_at))
                                        .small()
                                        .weak(),
                                );
                            });
                        }
                        ui.label(&message.content);
                        previous = Some(message);
                    }
                });
        });
    }

    fn call(&mut self, ui: &mut egui::Ui) {
        let Some(voice) = self.voice.clone() else {
            return;
        };
        self.update_textures(ui.ctx(), &voice);
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(key) = self.expanded.clone() {
                if let Some(tile) = voice.tiles.iter().find(|t| t.key == key) {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} · {}",
                            tile.name,
                            if tile.screen {
                                "Compartilhamento"
                            } else {
                                "Câmera"
                            }
                        ));
                        if ui.button("Reduzir").clicked() {
                            self.expanded = None;
                            self.set_fullscreen(ui.ctx(), false);
                        }
                        let label = if self.fullscreen {
                            "Sair da tela cheia"
                        } else {
                            "Tela cheia"
                        };
                        if ui.button(label).clicked() {
                            let full = !self.fullscreen;
                            self.set_fullscreen(ui.ctx(), full);
                        }
                    });
                    self.tile_volume(ui, &voice, &tile.user, tile.screen);
                    let size = ui.available_size();
                    self.video(ui, &key, size);
                    return;
                }
                self.expanded = None;
                self.set_fullscreen(ui.ctx(), false);
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    if voice.screen_pending {
                        ui.label("Aguardando início da captura / autorização do sistema…");
                    }
                    if voice.screen {
                        ui.label(
                            RichText::new(format!(
                                "🖥 Compartilhando · vídeo: {} · áudio: {}",
                                if voice.share_video {
                                    "ativo"
                                } else {
                                    "desativado"
                                },
                                if voice.share_audio {
                                    "ativo"
                                } else {
                                    "desativado"
                                }
                            ))
                            .color(ACCENT),
                        );
                        ui.add_space(6.0);
                    }
                    if !voice.tiles.is_empty() {
                        let width = ui.available_width();
                        let columns =
                            ((width / 260.0).floor() as usize).clamp(1, voice.tiles.len());
                        let tile_width = (width - 10.0 * (columns as f32 - 1.0)) / columns as f32;
                        let tile_size = Vec2::new(tile_width, tile_width * 9.0 / 16.0);
                        for row in voice.tiles.chunks(columns) {
                            ui.horizontal(|ui| {
                                for tile in row {
                                    ui.vertical(|ui| {
                                        let response = self.video(ui, &tile.key, tile_size);
                                        if response.on_hover_text("Clique para ampliar").clicked() {
                                            self.expanded = Some(tile.key.clone());
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}",
                                                tile.name,
                                                if tile.screen {
                                                    "Compartilhamento"
                                                } else {
                                                    "Câmera"
                                                }
                                            ))
                                            .small(),
                                        );
                                        self.tile_volume(ui, &voice, &tile.user, tile.screen);
                                    });
                                }
                            });
                        }
                        ui.add_space(12.0);
                    }
                    let columns = ((ui.available_width() / 226.0).floor() as usize).max(1);
                    for row in voice.participants.chunks(columns) {
                        ui.horizontal_top(|ui| {
                            for participant in row {
                                let stroke = if participant.speaking {
                                    egui::Stroke::new(2.0, ACCENT)
                                } else {
                                    egui::Stroke::new(1.0, Color32::from_gray(60))
                                };
                                egui::Frame::group(ui.style())
                                    .stroke(stroke)
                                    .inner_margin(12.0)
                                    .show(ui, |ui| {
                                        ui.set_width(190.0);
                                        ui.set_min_height(100.0);
                                        ui.vertical_centered(|ui| {
                                            let initial =
                                                participant.name.chars().next().unwrap_or('?');
                                            let (avatar, _) = ui.allocate_exact_size(
                                                Vec2::splat(32.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().circle_filled(
                                                avatar.center(),
                                                16.0,
                                                Color32::from_rgb(40, 65, 54),
                                            );
                                            ui.painter().text(
                                                avatar.center(),
                                                egui::Align2::CENTER_CENTER,
                                                initial,
                                                egui::FontId::proportional(18.0),
                                                ACCENT,
                                            );
                                            let suffix =
                                                if participant.local { " (você)" } else { "" };
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}{suffix}",
                                                    participant.name
                                                ))
                                                .strong(),
                                            );
                                            let state = if participant.muted {
                                                "🔇 Silenciado"
                                            } else if participant.speaking {
                                                "Falando"
                                            } else {
                                                "Na chamada"
                                            };
                                            ui.label(RichText::new(state).small().weak());
                                            if !participant.local {
                                                let user = user_of(&participant.identity);
                                                volume_control(
                                                    ui,
                                                    &self.cmd,
                                                    user,
                                                    AudioKind::Microphone,
                                                    participant.volume.microphone,
                                                );
                                                if participant.screen_audio {
                                                    volume_control(
                                                        ui,
                                                        &self.cmd,
                                                        user,
                                                        AudioKind::ScreenShare,
                                                        participant.volume.screen_share,
                                                    );
                                                }
                                            }
                                        });
                                    });
                            }
                        });
                        ui.add_space(8.0);
                    }
                });
        });
    }

    fn tile_volume(&self, ui: &mut egui::Ui, voice: &VoiceView, user: &str, screen: bool) {
        if !screen {
            return;
        }
        if let Some(p) = voice
            .participants
            .iter()
            .find(|p| user_of(&p.identity) == user && !p.local && p.screen_audio)
        {
            volume_control(
                ui,
                &self.cmd,
                user,
                AudioKind::ScreenShare,
                p.volume.screen_share,
            );
        }
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, full: bool) {
        if self.fullscreen != full {
            self.fullscreen = full;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(full));
        }
    }

    /// Desenha o vídeo mantendo a proporção dentro de `size`.
    fn video(&self, ui: &mut egui::Ui, key: &str, size: Vec2) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        ui.painter()
            .rect_filled(rect, 8.0, Color32::from_rgb(8, 14, 16));
        match self.textures.get(key) {
            Some((texture, _)) => {
                let [w, h] = texture.size();
                let scale = (rect.width() / w as f32).min(rect.height() / h as f32);
                let fitted = egui::Rect::from_center_size(
                    rect.center(),
                    Vec2::new(w as f32, h as f32) * scale,
                );
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(texture.id(), fitted, uv, Color32::WHITE);
            }
            None => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Carregando vídeo…",
                    egui::FontId::proportional(13.0),
                    Color32::GRAY,
                );
            }
        }
        response
    }

    /// Envia à GPU só os frames novos e descarta texturas de faixas encerradas.
    fn update_textures(&mut self, ctx: &egui::Context, voice: &VoiceView) {
        self.textures
            .retain(|key, _| voice.tiles.iter().any(|t| &t.key == key));
        let frames = self.frames.lock().unwrap();
        for tile in &voice.tiles {
            let Some(frame) = frames.get(&tile.key) else {
                continue;
            };
            let current = self.textures.get(&tile.key).map(|(_, seq)| *seq);
            if current == Some(frame.seq) {
                continue;
            }
            let image =
                egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.data);
            match self.textures.get_mut(&tile.key) {
                Some((texture, seq)) => {
                    texture.set(image, TextureOptions::LINEAR);
                    *seq = frame.seq;
                }
                None => {
                    let texture = ctx.load_texture(&tile.key, image, TextureOptions::LINEAR);
                    self.textures.insert(tile.key.clone(), (texture, frame.seq));
                }
            }
        }
    }

    fn audio_window(&mut self, ctx: &egui::Context) {
        let mut open = self.audio.open;
        egui::Window::new("Dispositivos de áudio")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                if self.demo {
                    ui.label("SIMULAÇÃO · dispositivos virtuais");
                }
                let audio = &mut self.audio;
                if audio.mics.is_empty() && audio.speakers.is_empty() {
                    ui.label("Nenhum dispositivo encontrado.");
                }
                let mut commands = vec![];
                for (label, devices, selected, is_mic) in [
                    ("Microfone", &audio.mics, &mut audio.mic, true),
                    ("Saída de áudio", &audio.speakers, &mut audio.speaker, false),
                ] {
                    let current = devices
                        .iter()
                        .find(|d| &d.id == selected)
                        .map_or("Padrão do sistema", |d| d.name.as_str());
                    egui::ComboBox::from_label(label)
                        .selected_text(current)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for device in devices {
                                if ui
                                    .selectable_label(&device.id == selected, &device.name)
                                    .clicked()
                                {
                                    *selected = device.id.clone();
                                    commands.push(if is_mic {
                                        Command::SelectMic(device.id.clone())
                                    } else {
                                        Command::SelectSpeaker(device.id.clone())
                                    });
                                }
                            }
                        });
                }
                ui.separator();
                let mut changed = ui
                    .checkbox(&mut audio.noise, "Supressão de ruído")
                    .changed();
                changed |= ui
                    .checkbox(&mut audio.echo, "Cancelamento de eco")
                    .changed();
                changed |= ui
                    .checkbox(&mut audio.agc, "Ganho automático do microfone")
                    .changed();
                if changed {
                    commands.push(Command::SetProcessing {
                        echo: audio.echo,
                        agc: audio.agc,
                        noise: audio.noise,
                    });
                }
                for cmd in commands {
                    let _ = self.cmd.send(cmd);
                }
            });
        self.audio.open = open;
    }

    fn error_banner(&mut self, ui: &mut egui::Ui) {
        let Some((message, since)) = self.error.clone() else {
            return;
        };
        if since.elapsed() > ERROR_TTL {
            self.error = None;
            return;
        }
        egui::Panel::top("error").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(message).color(Color32::from_rgb(0xff, 0xb4, 0xa8)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        self.error = None;
                    }
                });
            });
        });
        ui.ctx().request_repaint_after(ERROR_TTL);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::portable::ui_smoke(ui.ctx());
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
        if self.fullscreen && self.voice.is_none() {
            self.set_fullscreen(&ui.ctx().clone(), false);
        }
        if self.fullscreen && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.set_fullscreen(&ui.ctx().clone(), false);
        }
        self.error_banner(ui);
        if self.demo {
            egui::Panel::top("demo-banner").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(ACCENT, "MODO DEMO — OFFLINE");
                    ui.label("Todas as ações são simuladas");
                    for (label, state) in [
                        ("Conectar", VoiceStatus::Connected),
                        ("Desconectar", VoiceStatus::Connecting),
                        ("Reconectar", VoiceStatus::Reconnecting),
                    ] {
                        if ui.small_button(label).clicked() {
                            self.send(Command::DemoState(state));
                        }
                    }
                    if ui.small_button("Simular erro").clicked() {
                        self.send(Command::DemoError);
                    }
                });
            });
        }
        match self.screen {
            Screen::Loading => {
                egui::CentralPanel::default()
                    .show(ui, |ui| ui.centered_and_justified(|ui| ui.spinner()));
            }
            Screen::Login => self.login_screen(ui),
            Screen::Main => {
                if !self.fullscreen {
                    self.sidebar(ui);
                }
                match (self.view, self.voice.is_some()) {
                    (View::Call, true) => self.call(ui),
                    _ => self.chat(ui),
                }
                for command in self.share.show(&ui.ctx().clone(), self.demo) {
                    self.send(command);
                }
                if self.audio.open {
                    self.audio_window(&ui.ctx().clone());
                }
            }
        }
    }
}

fn merge(list: &mut Vec<ChatMessage>, incoming: Vec<ChatMessage>) {
    list.extend(incoming);
    list.sort_by_key(|m| m.id);
    list.dedup_by_key(|m| m.id);
}

/// "HH:MM" no fuso local; inclui a data quando não é de hoje.
fn format_time(timestamp: i64) -> String {
    fn local(t: i64) -> libc::tm {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = t as libc::time_t;
        #[cfg(windows)]
        unsafe {
            libc::localtime_s(&mut tm, &t)
        };
        #[cfg(not(windows))]
        unsafe {
            libc::localtime_r(&t, &mut tm)
        };
        tm
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (t, today) = (local(timestamp), local(now));
    if (t.tm_year, t.tm_yday) == (today.tm_year, today.tm_yday) {
        format!("{:02}:{:02}", t.tm_hour, t.tm_min)
    } else {
        format!(
            "{:02}/{:02} {:02}:{:02}",
            t.tm_mday,
            t.tm_mon + 1,
            t.tm_hour,
            t.tm_min
        )
    }
}

/// Ganho contínuo e mute independentes; mute preserva o valor para restaurar.
fn volume_control(
    ui: &mut egui::Ui,
    cmd: &UnboundedSender<Command>,
    user: &str,
    kind: AudioKind,
    mut volume: Volume,
) {
    ui.push_id((user, kind as u8), |ui| {
        ui.label(
            egui::RichText::new(match kind {
                AudioKind::Microphone => "🔊 Microfone",
                AudioKind::ScreenShare => "🔊 Compartilhamento",
            })
            .small(),
        );
        let mut percent = volume.gain * 100.0;
        let changed = ui
            .add(
                egui::Slider::new(&mut percent, 0.0..=150.0)
                    .suffix("%")
                    .step_by(1.0),
            )
            .changed();
        volume.gain = percent / 100.0;
        let mute = ui
            .small_button(if volume.muted {
                "🔇 Restaurar"
            } else {
                "Silenciar"
            })
            .clicked();
        if mute {
            volume.muted = !volume.muted;
        }
        if changed || mute {
            let _ = cmd.send(Command::SetVolume {
                user: user.into(),
                kind,
                volume,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_ui_renders_call_expansion_chat_and_modals_offline() {
        let (cmd, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = mpsc::channel();
        let (fixture, frames) = crate::demo::ui_fixture();
        let mut app = App::new(cmd, events, frames, true);
        for event in fixture {
            app.handle(event);
        }
        assert!(matches!(app.screen, Screen::Main));
        assert!(app.voice.is_some());
        let ctx = egui::Context::default();
        fn paint(ctx: &egui::Context, ui: impl FnMut(&mut egui::Ui)) -> bool {
            let mut output = ctx.run_ui(Default::default(), ui);
            let visible = !output.shapes.is_empty();
            output.textures_delta.clear();
            visible
        }
        let visible = paint(&ctx, |ui| {
            app.sidebar(ui);
            app.call(ui);
        });
        assert!(visible);
        let key = app.voice.as_ref().unwrap().tiles[0].key.clone();
        app.expanded = Some(key.clone());
        let _ = paint(&ctx, |ui| app.call(ui));
        assert_eq!(app.expanded.as_deref(), Some(key.as_str()));
        app.share.open = true;
        let request = match app.share.refresh() {
            Command::ListSources { request } => request,
            _ => unreachable!(),
        };
        app.handle(Event::Sources {
            request,
            result: Ok(vec![]),
        });
        let _ = paint(&ctx, |_ui| {
            app.share.show(&ctx, true);
        });
        assert!(app.share.open);
        app.share.open = false;
        app.audio.open = true;
        let _ = paint(&ctx, |_ui| app.audio_window(&ctx));
        app.handle(Event::Voice(None));
        assert!(app.expanded.is_none());
        assert!(!app.share.open);
        let _ = paint(&ctx, |ui| app.chat(ui));
        assert!(!app.messages["c0"].is_empty());
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::LoadMessages { .. })
        ));
        app.textures.clear();
        paint(&ctx, |_ui| {});
    }
}
