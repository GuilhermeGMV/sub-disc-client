//! Cliente nativo do kledisc: chat, voz e compartilhamento de tela.
// No Windows, o build de release não abre uma janela de console junto.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod admin_ui;
mod api;
mod audio;
mod backend;
mod bridge;
mod camera;
mod capture;
mod config;
mod demo;
mod model;
mod screen;
mod system_audio;
mod ui;
mod ui_camera;
mod ui_capture;
mod voice;
mod ws;

use bridge::{Frames, UiSink};

/// No Windows o release não tem console: o log vai para
/// `%APPDATA%\subdisc-client\kledisc.log` (anterior em `kledisc.log.old`).
/// Pânicos também são registrados, com local, antes do comportamento padrão.
fn init_logging(role: &'static str) {
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,subdisc_client=info"),
    );
    builder.format(move |out, record| {
        use std::io::Write;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        writeln!(
            out,
            "{}.{:03} [{role} {}] {} {}: {}",
            now.as_secs(),
            now.subsec_millis(),
            std::process::id(),
            record.level(),
            record.target(),
            record.args()
        )
    });
    if cfg!(windows)
        && role != "demo"
        && let Some(dir) = config::dir()
    {
        let path = dir.join("kledisc.log");
        let _ = std::fs::create_dir_all(&dir);
        // Só o app principal gira o arquivo; o subprocesso de áudio acrescenta.
        if role == "app" && std::fs::metadata(&path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    builder.init();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("pânico: {info}");
        default(info);
    }));
}

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--list-cameras") {
        match camera::list_devices() {
            Ok(devices) => println!("{}", serde_json::to_string(&devices).unwrap()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some(system_audio::WORKER_ARG) {
        init_logging("audio-worker");
        let args: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(system_audio::worker(&args));
    }
    if let Some(encoded) = std::env::args()
        .nth(1)
        .filter(|arg| arg == "--camera-worker")
        .and_then(|_| std::env::args().nth(2))
    {
        if let Err(error) = camera::worker(&encoded) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let demo = std::env::args().any(|arg| arg == "--demo" || arg == "--dev-ui");
    init_logging(if demo { "demo" } else { "app" });
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
            .with_title("kledisc")
            .with_app_id("kledisc")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/kledisc.png"))
                    .expect("application icon"),
            )
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    let handle = runtime.handle().clone();
    let task = std::sync::Arc::new(std::sync::Mutex::new(None));
    let task_slot = task.clone();
    let result = eframe::run_native(
        "kledisc",
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
