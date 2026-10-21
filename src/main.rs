//! Cliente nativo do Subdisc: chat, voz e compartilhamento de tela.
// No Windows, o build de release não abre uma janela de console junto.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod admin_ui;
mod api;
mod audio;
mod backend;
mod bridge;
mod capture;
mod config;
mod demo;
mod media_probe;
mod model;
mod platform_audio;
mod portable;
mod rtc;
mod screen;
mod ui;
mod ui_capture;
mod voice;
mod ws;
#[cfg(target_os = "linux")]
mod x11_capture;

use bridge::{Frames, UiSink};

fn main() -> eframe::Result {
    if let Err(error) = portable::configure() {
        portable::fatal(&error);
    }
    if let Some(path) = portable::report_path("--portable-check") {
        match portable::check() {
            Ok(report) => match portable::write_report(&path, &report) {
                Ok(()) => std::process::exit(0),
                Err(error) => portable::fatal(&error),
            },
            Err(error) => portable::fatal(&error),
        }
    }
    let demo = std::env::args().any(|arg| arg == "--demo" || arg == "--dev-ui");
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,subdisc_client=info"),
    )
    .init();
    // Poucas threads: o trabalho pesado de mídia roda nas threads do próprio WebRTC.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("subdisc-rt")
        .enable_all()
        .build()
        .expect("runtime tokio");
    if std::env::args().any(|arg| arg == "--media-probe") {
        if let Err(e) = runtime.block_on(media_probe::run()) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }
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
            *task_slot.lock().unwrap() = Some(if demo {
                handle.spawn(demo::run(cmd_rx, sink, frames.clone()))
            } else {
                handle.spawn(backend::run(cmd_rx, sink, frames.clone()))
            });
            Ok(Box::new(ui::App::new(cmd_tx, event_rx, frames, demo)))
        }),
    );
    // Fechar a janela solta o canal de comandos: o backend sai da chamada e termina.
    if let Some(task) = task.lock().unwrap().take() {
        let _ = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(3), task).await
        });
    }
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}
