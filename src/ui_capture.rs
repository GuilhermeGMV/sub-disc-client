//! Estado do modal compartilhado entre os backends real e demo.
use crate::bridge::Command;
use crate::capture::{CaptureKind, CaptureSourceInfo, ShareRequest};
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
            error: None,
        }
    }
}

impl ShareDialog {
    pub fn refresh(&mut self) -> Command {
        self.request += 1;
        self.loading = true;
        self.selected = None;
        self.error = None;
        Command::ListSources {
            request: self.request,
        }
    }

    pub fn receive(&mut self, request: u64, result: Result<Vec<CaptureSourceInfo>, String>) {
        if !self.open || request != self.request {
            return;
        }
        self.loading = false;
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

    pub fn show(&mut self, ctx: &egui::Context, demo: bool) -> Vec<Command> {
        if !self.open {
            return vec![];
        }
        let mut commands = vec![];
        let modal = egui::Modal::new(egui::Id::new("share-picker")).show(ctx, |ui| {
            ui.set_width(450.0);
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
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
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
            ui.add_enabled(demo, egui::Checkbox::new(&mut self.audio, "Enviar áudio"));
            ui.label(egui::RichText::new(if demo { "Áudio e vídeo sintéticos; não abre dispositivos." } else { "Áudio da tela indisponível: este capturador fornece apenas vídeo. Seu microfone permanece independente." }).small().weak());
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            ui.separator();
            let selection = self.selected.as_ref().filter(|s| s.kind == self.kind);
            let enabled = !self.loading && selection.is_some() && (self.video || (demo && self.audio));
            ui.horizontal(|ui| {
                if ui.add_enabled(enabled, egui::Button::new("Iniciar compartilhamento")).clicked()
                    && let Some(source) = selection {
                        commands.push(Command::StartScreen(ShareRequest { source: source.clone(), video: self.video, audio: self.audio }));
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
    #[test]
    fn cancelled_and_stale_enumerations_cannot_select_sources() {
        let mut modal = ShareDialog {
            open: true,
            ..Default::default()
        };
        modal.refresh();
        modal.refresh();
        modal.receive(1, Err("antigo".into()));
        assert!(modal.error.is_none());
        modal.open = false;
        modal.receive(
            2,
            Ok(vec![CaptureSourceInfo {
                id: 1,
                title: "Monitor".into(),
                kind: CaptureKind::Monitor,
            }]),
        );
        assert!(modal.sources.is_empty());
    }
}
