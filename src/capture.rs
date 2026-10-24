//! Contrato da seleção. Enumerar nunca inicia captura nem solicita mídia.
use livekit::webrtc::desktop_capturer::{
    DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Dono da thread nativa. Saída normal aguarda o join fora do executor;
/// cancelamento de uma future também sinaliza parada e agenda o join.
pub struct CaptureWorker {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureWorker {
    pub fn spawn(
        run: impl FnOnce(Arc<AtomicBool>, Arc<AtomicBool>) + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn_named("screen-capture", run)
    }

    pub fn spawn_named(
        name: &str,
        run: impl FnOnce(Arc<AtomicBool>, Arc<AtomicBool>) + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (stop_thread, done_thread) = (stop.clone(), done.clone());
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                struct Finished(Arc<AtomicBool>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::Release);
                    }
                }
                let _finished = Finished(done_thread.clone());
                run(stop_thread, done_thread);
            })
            .map_err(|_| "Não foi possível iniciar a captura.".to_owned())?;
        Ok(Self {
            stop,
            done,
            thread: Some(thread),
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub async fn close(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn_blocking(move || {
                    let _ = thread.join();
                });
            } else {
                let _ = thread.join();
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Monitor,
    Window,
    Portal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSourceInfo {
    pub id: u64,
    pub title: String,
    pub kind: CaptureKind,
}

/// Origem do áudio enviado junto com o compartilhamento. Capturar uma janela
/// não restringe o som a ela: o escopo é escolhido separadamente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioScope {
    /// Tudo o que o sistema reproduz, exceto o próprio cliente (as vozes da chamada).
    System,
    /// Somente o processo dono da janela selecionada e seus subprocessos.
    Application,
}

/// O que este cliente consegue capturar de áudio nesta máquina.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSupport {
    pub system: bool,
    pub application: bool,
}

/// `Err` traz o motivo exibido no modal quando o envio de áudio está desabilitado.
pub type AudioAvailability = Result<AudioSupport, String>;

#[derive(Clone, Debug)]
pub struct ShareRequest {
    /// Monitor/janela. Dispensável só para áudio do sistema sem vídeo.
    pub source: Option<CaptureSourceInfo>,
    pub video: bool,
    pub audio: Option<AudioScope>,
}

impl ShareRequest {
    pub fn validate(&self, audio: &AudioAvailability) -> Result<(), String> {
        if !self.video && self.audio.is_none() {
            return Err("Habilite vídeo ou áudio para compartilhar.".into());
        }
        if self.video && self.source.is_none() {
            return Err("Escolha uma tela ou janela para enviar vídeo.".into());
        }
        let Some(scope) = self.audio else {
            return Ok(());
        };
        let support = audio.as_ref().map_err(Clone::clone)?;
        match scope {
            AudioScope::System if !support.system => {
                Err("Captura do áudio do sistema indisponível neste computador.".into())
            }
            AudioScope::Application if !support.application => {
                Err("Captura do áudio de um aplicativo indisponível neste computador.".into())
            }
            AudioScope::Application
                if self.source.as_ref().map(|s| s.kind) != Some(CaptureKind::Window) =>
            {
                Err("Para enviar só o áudio de um aplicativo, escolha uma janela.".into())
            }
            _ => Ok(()),
        }
    }
}

pub fn uses_portal() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn source_type(kind: CaptureKind) -> Result<DesktopCaptureSourceType, String> {
    match kind {
        CaptureKind::Monitor => Ok(DesktopCaptureSourceType::Screen),
        CaptureKind::Window => Ok(DesktopCaptureSourceType::Window),
        #[cfg(target_os = "linux")]
        CaptureKind::Portal if uses_portal() => Ok(DesktopCaptureSourceType::Generic),
        _ => Err("Seletor do sistema indisponível nesta sessão.".into()),
    }
}

pub fn enumerate() -> Result<Vec<CaptureSourceInfo>, String> {
    if uses_portal() {
        return Ok(vec![CaptureSourceInfo {
            id: 0,
            title: "Escolher no portal do sistema…".into(),
            kind: CaptureKind::Portal,
        }]);
    }
    let mut result = vec![];
    for kind in [CaptureKind::Monitor, CaptureKind::Window] {
        let Some(capturer) = DesktopCapturer::new(DesktopCapturerOptions::new(source_type(kind)?))
        else {
            continue;
        };
        result.extend(
            capturer
                .get_source_list()
                .into_iter()
                .enumerate()
                .map(|(index, source)| {
                    let title = source.title();
                    CaptureSourceInfo {
                        id: source.id(),
                        title: if title.trim().is_empty() {
                            format!(
                                "{} {}",
                                if kind == CaptureKind::Monitor {
                                    "Monitor"
                                } else {
                                    "Janela"
                                },
                                index + 1
                            )
                        } else {
                            title
                        },
                        kind,
                    }
                }),
        );
    }
    if result.is_empty() {
        Err(
            "Nenhum monitor ou janela disponível. Verifique as permissões e atualize as fontes."
                .into(),
        )
    } else {
        Ok(result)
    }
}

pub fn selected<'a>(
    sources: &'a [CaptureSourceInfo],
    requested: &CaptureSourceInfo,
) -> Result<&'a CaptureSourceInfo, String> {
    sources
        .iter()
        .find(|source| *source == requested)
        .ok_or_else(|| {
            "A fonte foi fechada ou removida. Atualize as fontes e escolha novamente.".into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_never_falls_back_to_another_source() {
        let monitor = CaptureSourceInfo {
            id: 1,
            title: "Monitor 1".into(),
            kind: CaptureKind::Monitor,
        };
        let window = CaptureSourceInfo {
            id: 1,
            title: "Editor".into(),
            kind: CaptureKind::Window,
        };
        let sources = vec![monitor.clone(), window.clone()];
        assert_eq!(selected(&sources, &window).unwrap(), &window);
        assert!(selected(&[monitor], &window).is_err());
        let mut replaced = window.clone();
        replaced.title = "Outra janela".into();
        assert!(selected(&[replaced], &window).is_err());
    }

    #[test]
    fn media_modes_match_capability() {
        let screen = CaptureSourceInfo {
            id: 1,
            title: "Tela".into(),
            kind: CaptureKind::Monitor,
        };
        let window = CaptureSourceInfo {
            id: 2,
            title: "Player".into(),
            kind: CaptureKind::Window,
        };
        let full: AudioAvailability = Ok(AudioSupport {
            system: true,
            application: true,
        });
        let none: AudioAvailability = Err("sem suporte neste SO".into());
        let system_only: AudioAvailability = Ok(AudioSupport {
            system: true,
            application: false,
        });
        let request = |source: Option<&CaptureSourceInfo>, video, audio| ShareRequest {
            source: source.cloned(),
            video,
            audio,
        };
        let scopes = [
            None,
            Some(AudioScope::System),
            Some(AudioScope::Application),
        ];
        for video in [true, false] {
            for audio in scopes {
                let r = request(Some(&window), video, audio);
                assert_eq!(r.validate(&full).is_ok(), video || audio.is_some());
                assert_eq!(r.validate(&none).is_ok(), video && audio.is_none());
                assert_eq!(
                    r.validate(&system_only).is_ok(),
                    (video || audio.is_some()) && audio != Some(AudioScope::Application)
                );
            }
        }
        // O motivo da indisponibilidade chega intacto à interface.
        let denied = request(Some(&screen), true, Some(AudioScope::System)).validate(&none);
        assert_eq!(denied.unwrap_err(), "sem suporte neste SO");
        // Áudio do sistema sozinho dispensa a fonte de vídeo; vídeo, não.
        assert!(
            request(None, false, Some(AudioScope::System))
                .validate(&full)
                .is_ok()
        );
        assert!(request(None, true, None).validate(&full).is_err());
        assert!(
            request(None, true, Some(AudioScope::System))
                .validate(&full)
                .is_err()
        );
        // Áudio de aplicativo exige uma janela: monitor e "nenhuma fonte" são recusados.
        for source in [None, Some(&screen)] {
            assert!(
                request(source, false, Some(AudioScope::Application))
                    .validate(&full)
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn worker_cleanup_on_stop_failure_and_cancel() {
        use std::sync::atomic::AtomicUsize;
        let active = Arc::new(AtomicUsize::new(0));
        for _ in 0..16 {
            let count = active.clone();
            let mut worker = CaptureWorker::spawn(move |stop, _done| {
                count.fetch_add(1, Ordering::SeqCst);
                while !stop.load(Ordering::Acquire) {
                    std::thread::park_timeout(std::time::Duration::from_millis(5));
                }
                count.fetch_sub(1, Ordering::SeqCst);
            })
            .unwrap();
            worker.close().await;
            assert!(worker.finished());
            assert_eq!(active.load(Ordering::SeqCst), 0);
        }
        let mut failed =
            CaptureWorker::spawn(|_, _| { /* fonte removida / permissão negada */ }).unwrap();
        failed.close().await;
        assert!(failed.finished());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let cancelled = CaptureWorker::spawn(move |stop, _| {
            while !stop.load(Ordering::Acquire) {
                std::thread::park_timeout(std::time::Duration::from_millis(5));
            }
            let _ = tx.send(());
        })
        .unwrap();
        drop(cancelled);
        tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .unwrap()
            .unwrap();
    }
}
