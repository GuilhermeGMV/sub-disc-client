//! Compartilhamento de tela com o capturador do próprio libwebrtc. No Wayland ele
//! abre o seletor do portal (xdg-desktop-portal + PipeWire); no X11 e no Windows
//! captura exclusivamente o monitor ou janela confirmado pelo usuário.
use crate::capture::{self, CaptureKind, CaptureWorker, ShareRequest};
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const FPS: u64 = 15;
/// Tempo para a pessoa escolher a tela no seletor do portal.
const PICK_TIMEOUT: Duration = Duration::from_secs(120);

pub struct ScreenShare {
    worker: CaptureWorker,
    publication: Option<(LocalParticipant, TrackSid)>,
}

impl ScreenShare {
    /// Abre o seletor, espera o primeiro frame e publica a faixa de tela.
    pub async fn start(
        local: LocalParticipant,
        request: ShareRequest,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Self, String> {
        request.validate(false)?;
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 1920,
                height: 1080,
            },
            true,
        );
        let (ready_tx, ready_rx) = oneshot::channel();
        let capture_source = source.clone();
        let worker = CaptureWorker::spawn(move |stop, done| {
            capture_loop(capture_source, stop, done, ready_tx, request)
        })?;
        let mut share = Self {
            worker,
            publication: None,
        };
        let ready = tokio::select! {
            result = tokio::time::timeout(PICK_TIMEOUT, ready_rx) => result,
            _ = &mut cancel => return Err("Compartilhamento cancelado.".into()),
        };
        match ready {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => return Err(e),
            _ => {
                share.stop();
                return Err("O compartilhamento de tela não começou a tempo.".into());
            }
        }
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source));
        let options = TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_codec: VideoCodec::VP8,
            video_encoding: Some(VideoEncoding {
                max_bitrate: 2_500_000,
                max_framerate: FPS as f64,
            }),
            simulcast: false,
            ..Default::default()
        };
        match local.publish_track(LocalTrack::Video(track), options).await {
            Ok(publication) => {
                share.publication = Some((local, publication.sid()));
                if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                    share.close().await;
                    return Err("Compartilhamento cancelado.".into());
                }
                Ok(share)
            }
            Err(e) => {
                share.stop();
                log::warn!("falha ao publicar a tela: {e}");
                Err("Não foi possível publicar a tela.".into())
            }
        }
    }

    pub async fn close(mut self) {
        self.worker.close().await;
        if let Some((local, sid)) = self.publication.take() {
            let _ = local.unpublish_track(&sid).await;
        }
    }

    pub fn stop(&self) {
        self.worker.stop();
    }

    /// A captura terminou por conta própria (seletor cancelado, fonte encerrada).
    pub fn finished(&self) -> bool {
        self.worker.finished()
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.stop();
        if let Some((local, sid)) = self.publication.take() {
            tokio::spawn(async move {
                let _ = local.unpublish_track(&sid).await;
            });
        }
    }
}

type Ready = Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

fn capture_loop(
    source: NativeVideoSource,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    ready_tx: oneshot::Sender<Result<(), String>>,
    request: ShareRequest,
) {
    let ready: Ready = Arc::new(Mutex::new(Some(ready_tx)));
    let signal = |ready: &Ready, result: Result<(), String>| {
        if let Some(tx) = ready.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    };
    let kind = request.source.kind;
    // Reject forged/direct X11 requests in a Wayland session.
    if capture::uses_portal() != (kind == CaptureKind::Portal) {
        done.store(true, Ordering::Relaxed);
        signal(
            &ready,
            Err("Escolha a fonte no seletor autorizado do sistema.".into()),
        );
        return;
    }
    let source_type = match capture::source_type(kind) {
        Ok(kind) => kind,
        Err(error) => {
            done.store(true, Ordering::Relaxed);
            signal(&ready, Err(error));
            return;
        }
    };
    let mut options = DesktopCapturerOptions::new(source_type);
    options.set_include_cursor(true);
    let Some(mut capturer) = DesktopCapturer::new(options) else {
        done.store(true, Ordering::Relaxed);
        signal(
            &ready,
            Err("Captura indisponível ou permissão negada.".into()),
        );
        return;
    };
    let selected = if kind == CaptureKind::Portal {
        None
    } else {
        // Re-enumerate on the actual capturer: never silently capture another source.
        let sources = capturer.get_source_list();
        let selected = sources
            .into_iter()
            .enumerate()
            .find(|(index, s)| {
                let title = s.title();
                let title = if title.trim().is_empty() {
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
                };
                let info = capture::CaptureSourceInfo {
                    id: s.id(),
                    title,
                    kind,
                };
                capture::selected(std::slice::from_ref(&info), &request.source).is_ok()
            })
            .map(|(_, s)| s);
        if selected.is_none() {
            done.store(true, Ordering::Relaxed);
            signal(
                &ready,
                Err("A fonte foi fechada ou desconectada. Atualize a lista.".into()),
            );
            return;
        }
        selected
    };
    let callback = {
        let (ready, done) = (ready.clone(), done.clone());
        let started = Instant::now();
        move |result: Result<DesktopFrame, CaptureError>| match result {
            Ok(frame) => {
                if let Some(buffer) = to_i420(&frame) {
                    let timestamp_us = started.elapsed().as_micros() as i64;
                    let frame = VideoFrame {
                        rotation: VideoRotation::VideoRotation0,
                        timestamp_us,
                        frame_metadata: None,
                        buffer,
                    };
                    source.capture_frame(&frame);
                    signal(&ready, Ok(()));
                }
            }
            Err(CaptureError::Temporary) => {}
            Err(CaptureError::Permanent) => {
                done.store(true, Ordering::Relaxed);
                signal(&ready, Err("Compartilhamento de tela cancelado.".into()));
            }
        }
    };
    if !capturer.start_capture(selected, callback) {
        done.store(true, Ordering::Relaxed);
        signal(
            &ready,
            Err("A fonte selecionada não está mais disponível ou a permissão foi negada.".into()),
        );
        return;
    }
    let interval = Duration::from_millis(1000 / FPS);
    let mut checked = Instant::now();
    while !stop.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
        let tick = Instant::now();
        if kind != CaptureKind::Portal && checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            if !capturer
                .get_source_list()
                .iter()
                .any(|s| s.id() == request.source.id)
            {
                done.store(true, Ordering::Relaxed);
                signal(&ready, Err("A fonte compartilhada foi encerrada.".into()));
                break;
            }
        }
        capturer.capture_frame();
        std::thread::park_timeout(interval.saturating_sub(tick.elapsed()));
    }
    done.store(true, Ordering::Relaxed);
}

/// Frames do capturador vêm em BGRA ("ARGB" no libyuv).
fn to_i420(frame: &DesktopFrame) -> Option<I420Buffer> {
    let (width, height) = (frame.width(), frame.height());
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut buffer = I420Buffer::new(width as u32, height as u32);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    yuv_helper::argb_to_i420(
        frame.data(),
        frame.stride(),
        y,
        stride_y,
        u,
        stride_u,
        v,
        stride_v,
        width,
        height,
    );
    Some(buffer)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requer Xvfb e SUBDISC_TEST_CAPTURE=1; nunca executar no desktop pessoal"]
    async fn x11_sources_selection_removal_and_native_cleanup() {
        assert_eq!(std::env::var("SUBDISC_TEST_CAPTURE").as_deref(), Ok("1"));
        assert!(!capture::uses_portal());
        struct Window(std::process::Child);
        impl Drop for Window {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut window = Window(
            std::process::Command::new("xmessage")
                .args([
                    "-title",
                    "subdisc-capture-test",
                    "Conteúdo sintético autorizado para teste",
                ])
                .spawn()
                .unwrap(),
        );
        let sources = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // Xvfb has no WM: mark only our synthetic window as mapped/normal,
                // as a real window manager would (ICCCM WM_STATE).
                let _ = std::process::Command::new("xprop")
                    .args([
                        "-name",
                        "subdisc-capture-test",
                        "-f",
                        "WM_STATE",
                        "32c",
                        "-set",
                        "WM_STATE",
                        "1, 0",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let sources = capture::enumerate().unwrap_or_default();
                if sources.iter().any(|s| s.title == "subdisc-capture-test") {
                    break sources;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert!(sources.iter().any(|s| s.kind == CaptureKind::Monitor));
        let selected = sources
            .into_iter()
            .find(|s| s.title == "subdisc-capture-test")
            .unwrap();
        assert_eq!(selected.kind, CaptureKind::Window);
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 640,
                height: 480,
            },
            true,
        );
        let (tx, rx) = oneshot::channel();
        let request = ShareRequest {
            source: selected.clone(),
            video: true,
            audio: false,
        };
        let mut worker =
            CaptureWorker::spawn(move |stop, done| capture_loop(source, stop, done, tx, request))
                .unwrap();
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        window.0.kill().unwrap();
        window.0.wait().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !worker.finished() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        worker.close().await;

        // Removed source must fail before the first frame, with no default fallback.
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 640,
                height: 480,
            },
            true,
        );
        let (tx, rx) = oneshot::channel();
        let request = ShareRequest {
            source: selected,
            video: true,
            audio: false,
        };
        let mut worker =
            CaptureWorker::spawn(move |stop, done| capture_loop(source, stop, done, tx, request))
                .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), rx)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        worker.close().await;
        assert!(worker.finished());
    }
}
