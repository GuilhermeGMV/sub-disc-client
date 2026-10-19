//! Cliente nativo do Subdisc: chat, voz e compartilhamento de tela.
// No Windows, o build de release não abre uma janela de console junto.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod api;
mod backend;
mod bridge;
mod config;
mod model;
mod screen;
mod ui;
mod voice;
mod ws;

use bridge::{Frames, UiSink};

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,subdisc_client=info")).init();
    // Poucas threads: o trabalho pesado de mídia roda nas threads do próprio WebRTC.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("subdisc-rt")
        .enable_all()
        .build()
        .expect("runtime tokio");
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let frames = Frames::default();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Subdisc")
            .with_app_id("subdisc-client")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    let handle = runtime.handle().clone();
    let task = std::sync::Arc::new(std::sync::Mutex::new(None));
    let task_slot = task.clone();
    let result = eframe::run_native(
        "Subdisc",
        options,
        Box::new(move |cc| {
            let sink = UiSink::new(event_tx, cc.egui_ctx.clone());
            *task_slot.lock().unwrap() = Some(handle.spawn(backend::run(cmd_rx, sink, frames.clone())));
            Ok(Box::new(ui::App::new(cmd_tx, event_rx, frames)))
        }),
    );
    // Fechar a janela solta o canal de comandos: o backend sai da chamada e termina.
    if let Some(task) = task.lock().unwrap().take() {
        let _ = runtime.block_on(tokio::time::timeout(std::time::Duration::from_secs(3), task));
    }
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}
