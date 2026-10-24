use crate::bridge::{Command, Device};
use eframe::egui;

#[derive(Default)]
pub struct CameraDialog {
    pub open: bool,
    loading: bool,
    devices: Vec<Device>,
    selected: Option<Device>,
    error: Option<String>,
}
impl CameraDialog {
    pub fn refresh(&mut self) -> Command {
        self.loading = true;
        self.error = None;
        Command::ListCameras
    }
    pub fn receive(&mut self, result: Result<Vec<Device>, String>) {
        self.loading = false;
        match result {
            Ok(devices) => {
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|selected| !devices.contains(selected))
                {
                    self.selected = None;
                }
                self.devices = devices;
            }
            Err(error) => {
                self.devices.clear();
                self.selected = None;
                self.error = Some(error);
            }
        }
    }
    pub fn show(&mut self, ctx: &egui::Context, active: bool, pending: bool) -> Option<Command> {
        let mut open = self.open;
        let mut command = None;
        egui::Window::new("Câmera")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("Escolha o dispositivo de câmera");
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if self.loading {
                    ui.label("Procurando câmeras…");
                } else if self.devices.is_empty() {
                    ui.label(
                        "Nenhuma câmera encontrada. Conecte o dispositivo e atualize a lista.",
                    );
                }
                ui.add_enabled_ui(!self.loading && !pending, |ui| {
                    egui::ComboBox::from_id_salt("camera-device")
                        .selected_text(
                            self.selected
                                .as_ref()
                                .map_or("Selecione uma câmera", |d| d.name.as_str()),
                        )
                        .show_ui(ui, |ui| {
                            for device in &self.devices {
                                ui.selectable_value(
                                    &mut self.selected,
                                    Some(device.clone()),
                                    &device.name,
                                );
                            }
                        });
                    if ui.button("Atualizar câmeras").clicked() {
                        command = Some(self.refresh());
                    }
                    let label = if active {
                        "Trocar câmera"
                    } else {
                        "Ligar câmera"
                    };
                    if ui
                        .add_enabled(self.selected.is_some(), egui::Button::new(label))
                        .clicked()
                    {
                        command = self.selected.clone().map(Command::StartCamera);
                    }
                });
                if (active || pending)
                    && ui
                        .button(if pending {
                            "Cancelar abertura"
                        } else {
                            "Desligar câmera"
                        })
                        .clicked()
                {
                    command = Some(Command::StopCamera);
                }
                ui.label("A câmera só é aberta ao confirmar. Ao sair da chamada, ela é desligada.");
            });
        self.open = open;
        if matches!(command, Some(Command::StartCamera(_))) {
            self.open = false;
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_device_is_not_replaced_by_another_camera() {
        let selected = Device {
            id: "a".into(),
            name: "Camera A".into(),
        };
        let mut dialog = CameraDialog {
            selected: Some(selected.clone()),
            ..Default::default()
        };
        dialog.receive(Ok(vec![selected]));
        assert!(dialog.selected.is_some());
        dialog.receive(Ok(vec![Device {
            id: "b".into(),
            name: "Camera B".into(),
        }]));
        assert!(dialog.selected.is_none());
        dialog.receive(Err("Permission denied".into()));
        assert!(dialog.devices.is_empty());
        assert!(!dialog.loading);
    }
}
