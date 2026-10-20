//! App-local Windows runtime. Called before logger, Tokio, GUI or GStreamer threads.
use gstreamer::{self as gst, prelude::*};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Marker written by scripts/package-windows.ps1; its presence selects portable mode.
const MANIFEST: &str = "portable-runtime.json";
/// Relative paths, sizes, versions and hashes of every shipped file.
#[cfg(any(windows, test))]
const INVENTORY: &str = "files.json";
const PLUGIN_DIR: &str = "lib/gstreamer-1.0";
#[cfg(any(windows, test))]
const SCANNER: &str = "gst-plugin-scanner.exe";
#[cfg(any(windows, test))]
const CA_BUNDLE: &str = "etc/ssl/certs/ca-certificates.crt";

/// Bundle directory, set once the portable layout was detected and configured.
static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

/// How to repair a missing media component in the current mode.
pub fn repair_hint() -> &'static str {
    if root().is_some() {
        "O pacote portátil está incompleto ou corrompido: extraia novamente o ZIP completo em uma pasta nova."
    } else {
        "Instale as dependências de mídia (README, scripts/deps.sh)."
    }
}

#[cfg(any(windows, test))]
fn incomplete(file: &str) -> String {
    format!(
        "Pacote portátil incompleto: {file} ausente. Extraia novamente o ZIP completo em uma pasta nova."
    )
}

#[cfg(windows)]
pub fn configure() -> Result<(), String> {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = exe.parent().ok_or("Diretório do executável ausente")?;
    if !root.join(MANIFEST).is_file() {
        // Runtime files without the marker mean a damaged bundle, never a dev build.
        if root.join("gstreamer-1.0-0.dll").exists() || root.join(PLUGIN_DIR).exists() {
            return Err(incomplete(MANIFEST));
        }
        // Bare cargo builds retain the developer's GStreamer environment.
        return Ok(());
    }
    verify_inventory(root)?;
    // Initial imports already resolved from beside the EXE. Restrict later loads,
    // including plugin dependencies; never add cwd or user-controlled PATH entries.
    if unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA indisponível")?;
    let cache = local_path(Path::new(&local), "Subdisc/cache/gstreamer");
    std::fs::create_dir_all(&cache)
        .map_err(|e| format!("Não foi possível criar {}: {e}", cache.display()))?;
    let registry = cache.join(format!("registry-{:016x}.bin", location_key(root)));
    let system = system_directory()?;
    // SAFETY: main calls this before creating any threads or initializing native
    // libraries. All paths remain OsStr/UTF-16, including non-ASCII user names.
    unsafe {
        for key in [
            "GST_PLUGIN_PATH",
            "GST_PLUGIN_SYSTEM_PATH",
            "GST_PLUGIN_SCANNER",
            "GST_REGISTRY",
            "GST_PLUGIN_PATH_1_0",
            "GST_PLUGIN_SYSTEM_PATH_1_0",
            "GST_PLUGIN_SCANNER_1_0",
            "GST_REGISTRY_1_0",
            "GST_REGISTRY_UPDATE",
            "GST_REGISTRY_FORK",
            "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
            "GST_PLUGIN_FEATURE_RANK",
            "GST_PLUGIN_LOADING_WHITELIST",
            "GST_TRACERS",
            "GIO_EXTRA_MODULES",
            "GIO_USE_TLS",
        ] {
            std::env::remove_var(key);
        }
        // The bundle directory *is* the system plugin path: this replaces the
        // compiled-in/global directories without relying on an empty value.
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", local_path(root, PLUGIN_DIR));
        std::env::set_var("GST_PLUGIN_SCANNER_1_0", root.join(SCANNER));
        std::env::set_var("GST_REGISTRY_1_0", registry);
        std::env::set_var("GST_REGISTRY_UPDATE", "yes");
        std::env::set_var("GST_REGISTRY_FORK", "yes");
        // No GIO modules are shipped; this only keeps global ones from loading.
        std::env::set_var("GIO_MODULE_DIR", local_path(root, "lib/gio/modules"));
        std::env::set_var("OPENSSL_MODULES", local_path(root, "lib/ossl-modules"));
        std::env::set_var(
            "OPENSSL_CONF",
            local_path(root, "share/openssl/openssl.cnf"),
        );
        std::env::set_var("SSL_CERT_FILE", local_path(root, CA_BUNDLE));
        std::env::set_var("SSL_CERT_DIR", local_path(root, "etc/ssl/certs"));
        // The upstream scanner is a separate process using the standard search
        // order; GStreamer prepends the core DLL directory (the bundle root) to
        // this PATH when spawning it.
        std::env::set_var(
            "PATH",
            std::env::join_paths([root, &system]).map_err(|e| e.to_string())?,
        );
    }
    let _ = ROOT.set(root.to_path_buf());
    Ok(())
}

#[cfg(not(windows))]
pub fn configure() -> Result<(), String> {
    Ok(())
}

/// `relative` uses '/' (as in files.json); the result uses native separators.
fn local_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

#[cfg(windows)]
fn system_directory() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = [0u16; 512];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..length]).into())
}

/// Reads the packaged inventory and requires every listed file. Hashes are not
/// enforced: LGPL components may be replaced by compatible builds.
#[cfg(any(windows, test))]
fn verify_inventory(root: &Path) -> Result<Vec<String>, String> {
    for file in [INVENTORY, SCANNER, PLUGIN_DIR, CA_BUNDLE] {
        if !root.join(file).exists() {
            return Err(incomplete(file));
        }
    }
    let inventory: Vec<Value> = std::fs::read(root.join(INVENTORY))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| incomplete(&format!("{INVENTORY} válido")))?;
    let mut paths = Vec::with_capacity(inventory.len());
    let mut missing = Vec::new();
    for entry in &inventory {
        let path = entry["path"].as_str().unwrap_or_default();
        let relative = Path::new(path);
        if path.is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!("Inventário do pacote inválido: {path:?}"));
        }
        if !root.join(relative).is_file() {
            missing.push(path.to_owned());
        }
        paths.push(path.to_owned());
    }
    match missing.len() {
        0 => Ok(paths),
        1 => Err(incomplete(&missing[0])),
        n => Err(incomplete(&format!(
            "{} (e mais {} arquivos)",
            missing[..n.min(5)].join(", "),
            n - 1
        ))),
    }
}

#[cfg(any(windows, test))]
fn location_key(root: &Path) -> u64 {
    // Stable FNV-1a over the OS path and manifest; a moved/upgraded bundle gets a
    // fresh registry. No hashes are used to prevent replacing LGPL libraries.
    use std::hash::{Hash, Hasher};
    struct Fnv(u64);
    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
        }
    }
    let mut hash = Fnv(0xcbf29ce484222325);
    root.hash(&mut hash);
    if let Ok(bytes) = std::fs::read(root.join(MANIFEST)) {
        hash.write(&bytes);
    }
    hash.finish()
}

pub fn report_path(flag: &str) -> Option<PathBuf> {
    let mut args = std::env::args_os();
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

pub fn write_report(path: &Path, value: &Value) -> Result<(), String> {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Não foi possível gravar {}: {e}", path.display()))
}

/// Exercises actual codecs without opening microphones, speakers or a server.
/// `--portable-capture` adds one D3D11/WGC capture; it needs an interactive desktop.
pub fn check() -> Result<Value, String> {
    gst::init().map_err(|e| e.to_string())?;
    let portable = root();
    let specs: Value = serde_json::from_str(include_str!("../scripts/windows/runtime.json"))
        .map_err(|e| e.to_string())?;
    // Same form as the SDK's package version: nano only for GIT/prerelease builds.
    let core_version = match gst::version() {
        (major, minor, micro, 0) => format!("{major}.{minor}.{micro}"),
        (major, minor, micro, nano) => format!("{major}.{minor}.{micro}.{nano}"),
    };
    if let Some(root) = portable {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.join(MANIFEST)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if manifest["gstreamer_version"].as_str() != Some(core_version.as_str()) {
            return Err(format!(
                "Runtime GStreamer difere do SDK empacotado: {core_version}"
            ));
        }
    }
    let registry = gst::Registry::get();
    let mut plugins = Vec::new();
    for spec in specs["plugins"].as_array().ok_or("Manifesto inválido")? {
        if !cfg!(windows) && spec["windows"].as_bool() == Some(true) {
            continue;
        }
        let name = spec["name"].as_str().ok_or("Nome inválido")?;
        let plugin = registry
            .find_plugin(name)
            .ok_or_else(|| format!("Plugin ausente: {name}. {}", repair_hint()))?
            .load()
            .map_err(|e| format!("Plugin incompatível {name}: {e}"))?;
        if portable.is_some() && name != "nice" && plugin.version().as_str() != core_version {
            return Err(format!(
                "Plugin {name} tem versão diferente do core: {}",
                plugin.version()
            ));
        }
        let filename = plugin
            .filename()
            .ok_or_else(|| format!("Plugin sem arquivo: {name}"))?;
        if let Some(root) = portable
            && filename.parent() != Some(local_path(root, PLUGIN_DIR).as_path())
        {
            return Err(format!("Plugin externo: {}", filename.display()));
        }
        let license = plugin.license();
        let accepted = spec["licenses"]
            .as_array()
            .ok_or("Licenças ausentes no manifesto")?;
        if !accepted
            .iter()
            .any(|value| value.as_str() == Some(license.as_str()))
        {
            return Err(format!("Licença de plugin não revisada: {name}: {license}"));
        }
        for element in spec["elements"].as_array().ok_or("Elementos inválidos")? {
            let element = element.as_str().ok_or("Elemento inválido")?;
            let factory = gst::ElementFactory::find(element)
                .ok_or_else(|| format!("Elemento ausente: {element}. {}", repair_hint()))?;
            if factory.plugin_name().as_deref() != Some(name) {
                return Err(format!("Origem inesperada do elemento {element}"));
            }
            factory
                .create()
                .build()
                .map_err(|e| format!("Não foi possível criar {element}: {e}"))?;
        }
        plugins.push(json!({"name":name,"file":filename.file_name().map(|n| n.to_string_lossy()),"version":plugin.version().as_str(),
            "license":plugin.license().as_str(),"source":plugin.source().as_str(),"origin":plugin.origin().as_str()}));
    }
    let audio = roundtrip(
        "audiotestsrc num-buffers=32 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 ! opusenc ! rtpopuspay ! rtpopusdepay ! opusdec ! appsink name=result sync=false",
        None,
    )?;
    let video = roundtrip(
        "videotestsrc num-buffers=8 ! video/x-raw,width=160,height=120 ! videoconvert ! vp8enc deadline=1 ! rtpvp8pay ! rtpvp8depay ! vp8dec ! videoconvert ! video/x-raw,format=RGBA ! appsink name=result sync=false",
        Some(Some((160, 120))),
    )?;
    // Exercise live DSP processing and the playback mixer separately from codecs.
    roundtrip(
        "audiotestsrc num-buffers=32 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1,format=S16LE ! webrtcdsp echo-cancel=false ! appsink name=result sync=false",
        None,
    )?;
    roundtrip(
        "audiotestsrc num-buffers=32 ! audiomixer ! audioconvert ! audioresample ! audio/x-raw,rate=48000 ! webrtcechoprobe ! volume volume=0.5 ! audioconvert ! appsink name=result sync=false",
        None,
    )?;
    let capture = if std::env::args_os().any(|a| a == "--portable-capture") {
        if cfg!(windows) {
            json!(roundtrip(
                "d3d11screencapturesrc capture-api=wgc num-buffers=5 ! d3d11download ! videoconvert ! video/x-raw,format=RGBA ! appsink name=result sync=false",
                Some(None),
            )?)
        } else {
            json!("skipped: Windows only")
        }
    } else {
        Value::Null
    };
    // Loads the same device providers as the device menus, without opening devices.
    // Build/CI machines often have no devices or audio service, so the counts are
    // informative only and a blocking provider cannot stall the check.
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let counts = ["Audio/Source", "Audio/Sink"].map(|class| {
            let monitor = gst::DeviceMonitor::new();
            monitor.add_filter(Some(class), None);
            if monitor.start().is_err() {
                return None;
            }
            let count = monitor.devices().len();
            monitor.stop();
            Some(count)
        });
        let _ = sender.send(counts);
    });
    let devices = match receiver.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok([sources, sinks]) => json!({"sources": sources, "sinks": sinks}),
        Err(_) => json!("timeout"),
    };
    #[cfg(windows)]
    let modules = match portable {
        Some(root) => {
            // One spelling for the bundle and the modules (8.3 names, case,
            // junctions): e.g. CI temp directories are C:\Users\RUNNER~1\...
            let loaded: Vec<String> = loaded_modules()?
                .iter()
                .map(|module| canonical(Path::new(module)).to_string_lossy().into_owned())
                .collect();
            audit_modules(&canonical(root), &verify_inventory(root)?, &loaded)?
        }
        None => Value::Null,
    };
    #[cfg(not(windows))]
    let modules = Value::Null;
    Ok(
        json!({"ok":true,"portable":portable.is_some(),"gstreamer":gst::version_string().as_str(),
        "plugins":plugins,"audio_buffers":audio,"video_frames":video,"capture_frames":capture,
        "audio_devices":devices,"modules":modules,
        "registry":std::env::var_os("GST_REGISTRY_1_0").map(|p| p.to_string_lossy().into_owned())}),
    )
}

/// `video`: None for audio; Some(None) any frame size; Some(Some(size)) exact.
fn roundtrip(description: &str, video: Option<Option<(u32, u32)>>) -> Result<usize, String> {
    let pipeline = gst::parse::launch(description)
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "Pipeline inválido")?;
    struct Stop(gst::Pipeline);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = self.0.set_state(gst::State::Null);
        }
    }
    let _stop = Stop(pipeline.clone());
    let sink = pipeline
        .by_name("result")
        .ok_or("appsink ausente")?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| "appsink inválido")?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| e.to_string())?;
    let bus = pipeline.bus().ok_or("Barramento ausente")?;
    let start = std::time::Instant::now();
    let mut count = 0;
    while start.elapsed() < std::time::Duration::from_secs(15) {
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(200)) {
            let buffer = sample.buffer().ok_or("Buffer ausente")?;
            if buffer.size() == 0 {
                return Err("Buffer decodificado vazio".into());
            }
            if let Some(expected) = video {
                let info =
                    gstreamer_video::VideoInfo::from_caps(sample.caps().ok_or("Caps ausentes")?)
                        .map_err(|e| e.to_string())?;
                if info.width() == 0
                    || info.height() == 0
                    || expected.is_some_and(|size| size != (info.width(), info.height()))
                {
                    return Err("Dimensões incorretas".into());
                }
            }
            count += 1;
        }
        if let Some(error) = bus.pop_filtered(&[gst::MessageType::Error]) {
            return Err(format!("Erro no pipeline: {error:?}"));
        }
        if sink.is_eos() {
            return if count > 0 {
                Ok(count)
            } else {
                Err("Nenhuma mídia decodificada".into())
            };
        }
    }
    Err(format!("Timeout processando mídia: {description}"))
}

/// Full path with long names and resolved links, without the `\\?\` prefix.
#[cfg(windows)]
fn canonical(path: &Path) -> PathBuf {
    let Ok(full) = std::fs::canonicalize(path) else {
        return path.to_path_buf();
    };
    let text = full.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        full
    }
}

#[cfg(windows)]
fn loaded_modules() -> Result<Vec<String>, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::{
        LibraryLoader::GetModuleFileNameW, ProcessStatus::K32EnumProcessModules,
        Threading::GetCurrentProcess,
    };
    let size = std::mem::size_of::<HMODULE>();
    let mut modules: Vec<HMODULE> = vec![std::ptr::null_mut(); 512];
    loop {
        let mut needed = 0u32;
        // SAFETY: the buffer holds `modules.len()` handles and `needed` is valid.
        if unsafe {
            K32EnumProcessModules(
                GetCurrentProcess(),
                modules.as_mut_ptr(),
                (modules.len() * size) as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let count = needed as usize / size;
        if count <= modules.len() {
            modules.truncate(count);
            break;
        }
        modules.resize(count, std::ptr::null_mut());
    }
    let mut paths = Vec::with_capacity(modules.len());
    for module in modules {
        let mut buffer = vec![0u16; 32768];
        // SAFETY: valid module handle from this process and a sized UTF-16 buffer.
        let length =
            unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if length != 0 {
            paths.push(
                std::ffi::OsString::from_wide(&buffer[..length as usize])
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(paths)
}

/// Every module loaded so far (including runtime LoadLibrary calls made by the
/// plugins exercised above) must be bundled or a Windows component. A shipped or
/// GStreamer/CRT-like DLL loaded from elsewhere means the isolation failed.
/// Other third-party modules (e.g. antivirus or IME hooks) are only reported.
#[cfg(any(windows, test))]
fn audit_modules(root: &Path, inventory: &[String], modules: &[String]) -> Result<Value, String> {
    let normalize = |path: &str| path.replace('/', "\\").to_lowercase();
    let mut prefix = normalize(&root.to_string_lossy());
    if !prefix.ends_with('\\') {
        prefix.push('\\');
    }
    let windows = std::env::var_os("SystemRoot")
        .map(|p| normalize(&p.to_string_lossy()) + "\\")
        .unwrap_or_else(|| "c:\\windows\\".into());
    let shipped: std::collections::HashSet<String> = inventory
        .iter()
        .filter_map(|p| p.rsplit('/').next())
        .map(str::to_lowercase)
        .collect();
    let runtime_like = |name: &str| {
        let name = name.trim_start_matches("lib");
        [
            "gst",
            "glib",
            "gobject",
            "gio",
            "gmodule",
            "gthread",
            "nice",
            "orc",
            "opus",
            "vpx",
            "ffi",
            "intl",
            "pcre",
            "srtp",
            "ssl-",
            "crypto-",
            "vcruntime",
            "msvcp",
            "concrt",
            "webrtc",
        ]
        .iter()
        .any(|p| name.starts_with(p))
    };
    let (mut local, mut system, mut external) = (Vec::new(), 0, Vec::new());
    for module in modules {
        let path = normalize(module);
        let name = path.rsplit('\\').next().unwrap_or_default().to_owned();
        if let Some(relative) = path.strip_prefix(&prefix) {
            local.push(relative.replace('\\', "/"));
        } else if shipped.contains(&name)
            // Inbox components (e.g. msvcp_win.dll) are fine; a GStreamer- or
            // CRT-like DLL outside Windows comes from a global installation.
            || (!path.starts_with(&windows) && runtime_like(&name))
        {
            return Err(format!(
                "DLL carregada fora do pacote: {module}. {}",
                repair_hint()
            ));
        } else if path.starts_with(&windows) {
            system += 1;
        } else {
            external.push(module.clone());
        }
    }
    local.sort();
    Ok(json!({"bundle":local,"windows":system,"external":external}))
}

/// Explicit automation hook: only enabled with --demo/--dev-ui and an output path.
pub fn ui_smoke(ctx: &eframe::egui::Context) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FRAMES: AtomicUsize = AtomicUsize::new(0);
    static OUTPUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    let output = OUTPUT.get_or_init(|| {
        if std::env::args_os().any(|a| a == "--demo" || a == "--dev-ui") {
            report_path("--portable-ui-smoke")
        } else {
            None
        }
    });
    if let Some(path) = output {
        if FRAMES.fetch_add(1, Ordering::Relaxed) == 10 {
            if let Err(e) = write_report(path, &json!({"ok":true,"demo_frames":10})) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Startup failure: the release build has no console, so show a dialog, or
/// write the requested report when running the automated check.
pub fn fatal(message: &str) -> ! {
    eprintln!("{message}");
    if let Some(path) = report_path("--portable-check") {
        let _ = write_report(&path, &json!({"ok": false, "error": message}));
        std::process::exit(1);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let text: Vec<u16> = std::ffi::OsStr::new(message)
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                windows_sys::core::w!("Subdisc — runtime"),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_changes_on_move_or_manifest_change() {
        let dir = tempfile::tempdir().unwrap();
        let before = location_key(dir.path());
        std::fs::write(dir.path().join(MANIFEST), "v1").unwrap();
        let first = location_key(dir.path());
        assert_ne!(before, first);
        assert_eq!(first, location_key(dir.path()));
        assert_ne!(first, location_key(&dir.path().join("moved ü")));
        std::fs::write(dir.path().join(MANIFEST), "v2").unwrap();
        assert_ne!(first, location_key(dir.path()));
    }

    #[test]
    fn inventory_names_missing_files_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pasta ç 中");
        std::fs::create_dir_all(root.join("lib/gstreamer-1.0")).unwrap();
        std::fs::create_dir_all(root.join("etc/ssl/certs")).unwrap();
        for file in [SCANNER, CA_BUNDLE, "lib/gstreamer-1.0/gstwebrtc.dll"] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        assert!(verify_inventory(&root).unwrap_err().contains(INVENTORY));
        let write = |paths: &[&str]| {
            let entries: Vec<_> = paths.iter().map(|p| json!({"path": p})).collect();
            std::fs::write(root.join(INVENTORY), serde_json::to_vec(&entries).unwrap()).unwrap();
        };
        write(&[SCANNER, "lib/gstreamer-1.0/gstwebrtc.dll"]);
        assert_eq!(verify_inventory(&root).unwrap().len(), 2);
        write(&[SCANNER, "nice-10.dll"]);
        let error = verify_inventory(&root).unwrap_err();
        assert!(
            error.contains("nice-10.dll") && error.contains("ZIP"),
            "{error}"
        );
        for escape in ["../outside.dll", "/abs.dll", "lib/../../x.dll"] {
            write(&[escape]);
            assert!(verify_inventory(&root).unwrap_err().contains("inválido"));
        }
        std::fs::remove_file(root.join(SCANNER)).unwrap();
        assert!(verify_inventory(&root).unwrap_err().contains(SCANNER));
    }

    #[test]
    fn module_audit_rejects_runtime_loaded_from_outside_the_bundle() {
        let root = Path::new(r"C:\Users\Zoë\Subdisc portable");
        let inventory = vec![
            "gstreamer-1.0-0.dll".to_owned(),
            "vcruntime140.dll".to_owned(),
            "lib/gstreamer-1.0/gstwebrtc.dll".to_owned(),
        ];
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let ok = [
            r"C:\Users\Zoë\Subdisc portable\subdisc-client.exe".to_owned(),
            r"c:\users\zoë\subdisc portable\lib/gstreamer-1.0\gstwebrtc.dll".to_owned(),
            format!(r"{windows}\System32\kernel32.dll"),
            format!(r"{windows}\System32\msvcp_win.dll"),
            r"C:\Program Files\Antivirus\hook64.dll".to_owned(),
        ];
        let report = audit_modules(root, &inventory, &ok).unwrap();
        assert_eq!(report["bundle"].as_array().unwrap().len(), 2);
        assert_eq!(report["windows"], 2);
        assert_eq!(report["external"].as_array().unwrap().len(), 1);
        for bad in [
            format!(r"{windows}\System32\vcruntime140.dll"),
            r"C:\gstreamer\1.0\msvc_x86_64\bin\gstreamer-1.0-0.dll".to_owned(),
            r"C:\gstreamer\1.0\msvc_x86_64\lib\gstreamer-1.0\gstnice.dll".to_owned(),
            r"C:\Users\Zoë\Subdisc portable2\glib-2.0-0.dll".to_owned(),
        ] {
            assert!(audit_modules(root, &inventory, &[bad]).is_err());
        }
    }

    /// The packaged plugin list is derived from the real pipelines: every element
    /// named in the client's pipeline strings and factory lookups must be listed
    /// in scripts/windows/runtime.json (Linux-only capture elements excepted).
    #[test]
    fn runtime_spec_covers_every_pipeline_element() {
        let specs: Value =
            serde_json::from_str(include_str!("../scripts/windows/runtime.json")).unwrap();
        let listed: std::collections::HashSet<&str> = specs["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["elements"].as_array().unwrap())
            .map(|e| e.as_str().unwrap())
            .collect();
        let sources = [
            include_str!("rtc.rs"),
            include_str!("platform_audio.rs"),
            include_str!("screen.rs"),
            include_str!("media_probe.rs"),
            include_str!("voice.rs"),
            include_str!("capture.rs"),
            include_str!("portable.rs"),
        ];
        let linux_only = ["pipewiresrc", "ximagesrc"];
        let mut used = std::collections::BTreeSet::new();
        for source in sources {
            // '\u{22}' is a double quote, written so this file stays parseable here.
            for (index, literal) in source.split('\u{22}').enumerate() {
                if index % 2 == 0 {
                    continue;
                }
                let pipeline = literal.contains('!')
                    || literal
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|w| w.contains('='));
                if pipeline {
                    for segment in literal.split('!') {
                        let mut segment = segment.trim();
                        while let Some(rest) = segment.strip_prefix('{') {
                            segment = rest.split_once('}').map_or("", |(_, r)| r).trim();
                        }
                        let token = segment.split_whitespace().next().unwrap_or_default();
                        if token
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                            && token.chars().any(|c| c.is_ascii_lowercase())
                        {
                            used.insert(token.to_owned());
                        }
                    }
                }
            }
            for call in ["ElementFactory::make(", "ElementFactory::find("] {
                for rest in source.split(call).skip(1) {
                    let head = rest.split(')').next().unwrap_or_default();
                    for (index, word) in head.split('\u{22}').enumerate() {
                        if index % 2 == 1
                            && !word.is_empty()
                            && word
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        {
                            used.insert(word.to_owned());
                        }
                    }
                }
            }
        }
        for required in [
            "webrtcbin",
            "opusenc",
            "vp8dec",
            "audiomixer",
            "d3d11download",
        ] {
            assert!(used.contains(required), "extractor missed {required}");
        }
        let missing: Vec<_> = used
            .iter()
            .filter(|e| !listed.contains(e.as_str()) && !linux_only.contains(&e.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "add to runtime.json: {missing:?} (found {used:?})"
        );
    }
}
