//! Estado do modal compartilhado entre os backends real e demo.
use crate::bridge::Command;
use crate::capture::{AudioAvailability, AudioScope, CaptureKind, CaptureSourceInfo, ShareRequest};
use eframe::egui;

pub struct ShareDialog {
    pub open: bool,
    request: u64,
    loading: bool,
    sources: Vec<CaptureSourceInfo>,
    selected: Option<CaptureSourceInfo>,
    kind: CaptureKind,
    video: bool,
    audio: bool,
    scope: AudioScope,
    /// `None` enquanto a verificação do backend não chegou.
    audio_support: Option<AudioAvailability>,
    error: Option<String>,
}

impl Default for ShareDialog {
    fn default() -> Self {
        Self {
            open: false,
            request: 0,
            loading: false,
            sources: vec![],
            selected: None,
            kind: CaptureKind::Monitor,
            video: true,
            audio: false,
            scope: AudioScope::System,
            audio_support: None,
            error: None,
        }
    }
}

impl ShareDialog {
    pub fn refresh(&mut self) -> Command {
        self.request += 1;
        self.loading = true;
        self.selected = None;
        self.audio_support = None;
        self.error = None;
        Command::ListSources {
            request: self.request,
        }
    }

    pub fn receive(
        &mut self,
        request: u64,
        result: Result<Vec<CaptureSourceInfo>, String>,
        audio: AudioAvailability,
    ) {
        if !self.open || request != self.request {
            return;
        }
        self.loading = false;
        if audio.is_err() {
            self.audio = false;
        }
        self.audio_support = Some(audio);
        match result {
            Ok(sources) => {
                if sources.iter().any(|s| s.kind == CaptureKind::Portal) {
                    self.kind = CaptureKind::Portal;
                }
                self.sources = sources;
            }
            Err(error) => {
                self.sources.clear();
                self.error = Some(error);
            }
        }
    }

    fn audio_available(&self) -> bool {
        matches!(&self.audio_support, Some(Ok(s)) if s.system || s.application)
    }

    fn application_available(&self) -> bool {
        self.kind == CaptureKind::Window
            && matches!(&self.audio_support, Some(Ok(s)) if s.application)
    }

    /// Pedido montado a partir das escolhas, já validado como o backend fará.
    fn share_request(&self) -> Result<ShareRequest, String> {
        if self.loading {
            return Err("Carregando fontes…".into());
        }
        let request = ShareRequest {
            source: self.selected.clone().filter(|s| s.kind == self.kind),
            video: self.video,
            audio: (self.audio && self.audio_available()).then_some(self.scope),
        };
        let support = self
            .audio_support
            .clone()
            .unwrap_or_else(|| Err("Verificando o áudio do sistema…".into()));
        request.validate(&support)?;
        Ok(request)
    }

    pub fn show(&mut self, ctx: &egui::Context, demo: bool) -> Vec<Command> {
        if !self.open {
            return vec![];
        }
        let mut commands = vec![];
        let modal = egui::Modal::new(egui::Id::new("share-picker")).show(ctx, |ui| {
            ui.set_width(470.0);
            ui.heading("Compartilhar");
            if demo { ui.label("SIMULAÇÃO — nenhuma mídia será capturada ou publicada."); }
            ui.separator();
            ui.strong("Tipo de fonte");
            ui.horizontal(|ui| {
                if self.kind == CaptureKind::Portal {
                    ui.label("Monitor ou janela pelo portal do sistema");
                } else {
                    ui.selectable_value(&mut self.kind, CaptureKind::Monitor, "Tela inteira");
                    ui.selectable_value(&mut self.kind, CaptureKind::Window, "Janela específica");
                }
                ui.add_enabled(false, egui::Button::new("Aba do navegador"));
            });
            ui.label(egui::RichText::new("Abas indisponíveis: requer integração com getDisplayMedia no navegador. Capturar uma janela não isola uma aba.").small().weak());
            ui.separator();
            ui.strong("Fonte");
            if self.loading { ui.spinner(); }
            if !self.loading && !self.sources.iter().any(|s| s.kind == self.kind) {
                ui.label("Nenhuma fonte deste tipo disponível. Atualize a lista ou escolha outro tipo.");
            }
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for source in self.sources.iter().filter(|s| s.kind == self.kind) {
                    if ui.selectable_label(self.selected.as_ref() == Some(source), &source.title).clicked() {
                        self.selected = Some(source.clone());
                    }
                }
            });
            if ui.button("Atualizar fontes").clicked() { commands.push(self.refresh()); }
            ui.separator();
            ui.strong("Mídia enviada");
            ui.checkbox(&mut self.video, "Enviar vídeo");
            let audio_available = self.audio_available();
            ui.add_enabled(audio_available, egui::Checkbox::new(&mut self.audio, "Enviar áudio"));
            match &self.audio_support {
                None => { ui.label(egui::RichText::new("Verificando o áudio do sistema…").small().weak()); }
                Some(Err(reason)) => { ui.label(egui::RichText::new(reason).small().weak()); }
                Some(Ok(_)) if self.audio => {
                    if self.scope == AudioScope::Application && !self.application_available() {
                        self.scope = AudioScope::System;
                    }
                    ui.indent("share-audio-scope", |ui| {
                        ui.radio_value(&mut self.scope, AudioScope::System, "Todo o som do computador (exceto o kledisc)");
                        ui.add_enabled_ui(self.application_available(), |ui| {
                            ui.radio_value(&mut self.scope, AudioScope::Application, "Somente o aplicativo da janela selecionada");
                        });
                        let note = match self.scope {
                            AudioScope::System => "Atenção: transmite também sons de outros aplicativos e as notificações do Windows. As vozes desta chamada não são reenviadas.",
                            AudioScope::Application => "Inclui os subprocessos do aplicativo (ex.: o áudio do navegador). Apps da Microsoft Store podem não ser capturados por esta via.",
                        };
                        ui.label(egui::RichText::new(note).small().color(egui::Color32::from_rgb(0xe0, 0xb0, 0x60)));
                        if self.kind != CaptureKind::Window {
                            ui.label(egui::RichText::new("Para enviar o som de um único aplicativo, escolha \"Janela específica\".").small().weak());
                        }
                    });
                }
                Some(Ok(_)) => {}
            }
            ui.label(egui::RichText::new(if demo { "Áudio e vídeo sintéticos; não abre dispositivos." } else { "Seu microfone continua sendo uma faixa separada, com mudo e volume próprios." }).small().weak());
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            ui.separator();
            let request = self.share_request();
            if let Err(reason) = &request && !self.loading {
                ui.label(egui::RichText::new(reason).small().weak());
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(request.is_ok(), egui::Button::new("Iniciar compartilhamento")).clicked()
                    && let Ok(request) = request {
                        commands.push(Command::StartScreen(request));
                        self.open = false;
                }
                if ui.button("Cancelar").clicked() { self.open = false; }
            });
        });
        if modal.should_close() {
            self.open = false;
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::AudioSupport;

    fn source(id: u64, kind: CaptureKind) -> CaptureSourceInfo {
        CaptureSourceInfo {
            id,
            title: format!("fonte {id}"),
            kind,
        }
    }

    fn full() -> AudioAvailability {
        Ok(AudioSupport {
            system: true,
            application: true,
        })
    }

    #[test]
    fn cancelled_and_stale_enumerations_cannot_select_sources() {
        let mut modal = ShareDialog {
            open: true,
            ..Default::default()
        };
        modal.refresh();
        modal.refresh();
        modal.receive(1, Err("antigo".into()), full());
        assert!(modal.error.is_none());
        assert!(modal.audio_support.is_none());
        modal.open = false;
        modal.receive(2, Ok(vec![source(1, CaptureKind::Monitor)]), full());
        assert!(modal.sources.is_empty());
    }

    #[test]
    fn audio_modes_follow_backend_support_and_selection() {
        let mut modal = ShareDialog {
            open: true,
            ..Default::default()
        };
        let request = match modal.refresh() {
            Command::ListSources { request } => request,
            _ => unreachable!(),
        };
        let sources = vec![
            source(1, CaptureKind::Monitor),
            source(2, CaptureKind::Window),
        ];
        // Sem suporte (Linux, Windows antigo, sem saída): marcar áudio não o envia.
        modal.receive(
            request,
            Ok(sources.clone()),
            Err("sem saída de áudio".into()),
        );
        assert!(!modal.audio_available());
        modal.audio = true;
        modal.selected = Some(sources[0].clone());
        let r = modal.share_request().unwrap();
        assert!(r.video && r.audio.is_none());
        modal.video = false;
        assert!(modal.share_request().is_err());

        // Com suporte: só vídeo, vídeo + áudio do sistema e só áudio.
        let request = match modal.refresh() {
            Command::ListSources { request } => request,
            _ => unreachable!(),
        };
        assert!(modal.share_request().is_err(), "carregando");
        modal.receive(request, Ok(sources.clone()), full());
        modal.selected = Some(sources[0].clone());
        modal.video = true;
        modal.audio = false;
        assert_eq!(modal.share_request().unwrap().audio, None);
        modal.audio = true;
        let r = modal.share_request().unwrap();
        assert_eq!((r.video, r.audio), (true, Some(AudioScope::System)));
        modal.video = false;
        modal.selected = None;
        let r = modal.share_request().unwrap();
        assert_eq!(
            (r.video, r.audio, r.source),
            (false, Some(AudioScope::System), None)
        );

        // Áudio de aplicativo exige janela; com monitor o pedido é recusado.
        modal.scope = AudioScope::Application;
        assert!(!modal.application_available());
        assert!(modal.share_request().is_err());
        modal.kind = CaptureKind::Window;
        modal.selected = Some(sources[1].clone());
        assert!(modal.application_available());
        let r = modal.share_request().unwrap();
        assert_eq!(r.audio, Some(AudioScope::Application));
        assert_eq!(r.source.unwrap().kind, CaptureKind::Window);
        // Uma janela selecionada antes de trocar para monitor não vaza no pedido.
        modal.kind = CaptureKind::Monitor;
        modal.video = true;
        assert!(modal.share_request().is_err());
    }
}
