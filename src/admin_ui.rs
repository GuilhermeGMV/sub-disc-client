//! Administration is available only to admins; the server independently authorizes every request.
use crate::{
    bridge::Command,
    model::{Snapshot, User},
};
use eframe::egui;
use reqwest::Method;
use serde_json::json;

#[derive(Default)]
pub struct AdminPanel {
    pub open: bool,
    busy: bool,
    status: String,
    search: String,
    editing: Option<String>,
    username: String,
    password: String,
    admin_role: bool,
    channel_name: String,
    voice: bool,
    confirm: Option<(String, String)>,
}
impl AdminPanel {
    pub fn finish(&mut self, result: Result<(), String>) {
        self.busy = false;
        self.status = match result {
            Ok(()) => {
                self.password.clear();
                self.username.clear();
                self.editing = None;
                self.channel_name.clear();
                "Alteração salva.".into()
            }
            Err(message) => message,
        };
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        snapshot: &Snapshot,
        user: Option<&User>,
        demo: bool,
    ) -> Option<Command> {
        if !user.is_some_and(|u| u.role == "admin") {
            self.open = false;
            return None;
        }
        let mut open = self.open;
        let mut command = None;
        egui::Window::new("Administração").open(&mut open).default_width(640.0).show(ctx, |ui| {
            if !self.status.is_empty() { ui.label(&self.status); }
            if demo { ui.label("Administração indisponível no demo offline."); }
            egui::ScrollArea::vertical().max_height(650.0).show(ui, |ui| {
                ui.add_enabled_ui(!self.busy && !demo, |ui| {
                    ui.heading("Usuários");
                    ui.horizontal(|ui| { ui.label("Buscar"); ui.text_edit_singleline(&mut self.search); });
                    for account in snapshot.users.iter().filter(|u| u.username.contains(&self.search.to_lowercase())) {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("{} · {}", account.username, account.role));
                            if ui.button("Editar").clicked() {
                                self.editing = Some(account.id.clone());
                                self.username = account.username.clone();
                                self.admin_role = account.role == "admin";
                                self.password.clear();
                            }
                            if snapshot.voice.iter().any(|v| v.user_id == account.id) && ui.button("Tirar da chamada").clicked() {
                                command = Some(Command::Admin { method: Method::POST, path: format!("/users/{}/disconnect", account.id), body: Some(json!({})) });
                            }
                            if ui.button("Apagar conta").clicked() {
                                self.confirm = Some((format!("/users/{}", account.id), format!("Apagar a conta de {}? Suas mensagens e convites também serão apagados.", account.username)));
                            }
                        });
                    }
                    ui.separator();
                    ui.heading(if self.editing.is_some() { "Editar usuário" } else { "Criar usuário" });
                    ui.label("Usuário (3–32 letras, números, _ ou -)");
                    ui.text_edit_singleline(&mut self.username);
                    ui.label(if self.editing.is_some() { "Nova senha (vazia mantém a atual; alterar encerra as sessões)" } else { "Senha (12–128 bytes)" });
                    ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.admin_role, false, "member");
                        ui.radio_value(&mut self.admin_role, true, "admin");
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Salvar usuário").clicked() {
                            let (method, path) = self.editing.as_ref().map_or((Method::POST, "/users".into()), |id| (Method::PATCH, format!("/users/{id}")));
                            command = Some(Command::Admin { method, path, body: Some(json!({"username":self.username, "password":self.password, "role":if self.admin_role {"admin"} else {"member"}})) });
                        }
                        if self.editing.is_some() && ui.button("Cancelar edição").clicked() {
                            self.editing = None; self.username.clear(); self.password.clear(); self.admin_role = false;
                        }
                    });
                    ui.separator();
                    ui.heading("Canais de texto e salas de voz");
                    ui.text_edit_singleline(&mut self.channel_name);
                    ui.horizontal(|ui| { ui.radio_value(&mut self.voice, false, "Texto"); ui.radio_value(&mut self.voice, true, "Voz"); });
                    if ui.button("Criar canal").clicked() {
                        command = Some(Command::Admin { method: Method::POST, path: "/channels".into(), body: Some(json!({"name":self.channel_name,"kind":if self.voice {"voice"} else {"text"}})) });
                    }
                    for channel in &snapshot.channels {
                        ui.horizontal(|ui| {
                            ui.label(format!("{:?} · {}", channel.kind, channel.name));
                            if ui.button("Excluir canal").clicked() { self.confirm = Some((format!("/channels/{}", channel.id), format!("Excluir {} e todo o histórico?", channel.name))); }
                        });
                    }
                    if let Some((path, message)) = self.confirm.clone() {
                        ui.separator(); ui.label(message);
                        ui.horizontal(|ui| {
                            if ui.button("Confirmar exclusão permanente").clicked() {
                                command = Some(Command::Admin { method: Method::DELETE, path, body: None }); self.confirm = None;
                            }
                            if ui.button("Cancelar").clicked() { self.confirm = None; }
                        });
                    }
                });
            });
        });
        self.open = open;
        if command.is_some() {
            self.busy = true;
            self.status = "Salvando…".into();
        }
        command
    }
}
