//! Compartilhamento de tela com o capturador do próprio libwebrtc. No Wayland ele
//! abre o seletor do portal (xdg-desktop-portal + PipeWire); no X11 e no Windows
//! captura a tela principal.
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
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
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl ScreenShare {
    /// Abre o seletor, espera o primeiro frame e publica a faixa de tela.
    pub async fn start(local: LocalParticipant) -> Result<(Self, TrackSid), String> {
        let source = NativeVideoSource::new(VideoResolution { width: 1920, height: 1080 }, true);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = oneshot::channel();
        {
            let (source, stop, done) = (source.clone(), stop.clone(), done.clone());
            std::thread::Builder::new()
                .name("screen-capture".into())
                .spawn(move || capture_loop(source, stop, done, ready_tx))
                .map_err(|_| "Não foi possível iniciar a captura de tela.".to_owned())?;
        }
        let share = Self { stop, done };
        match tokio::time::timeout(PICK_TIMEOUT, ready_rx).await {
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
            video_encoding: Some(VideoEncoding { max_bitrate: 2_500_000, max_framerate: FPS as f64 }),
            simulcast: false,
            ..Default::default()
        };
        match local.publish_track(LocalTrack::Video(track), options).await {
            Ok(publication) => Ok((share, publication.sid())),
            Err(e) => {
                share.stop();
                log::warn!("falha ao publicar a tela: {e}");
                Err("Não foi possível publicar a tela.".into())
            }
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// A captura terminou por conta própria (seletor cancelado, fonte encerrada).
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.stop();
    }
}

type Ready = Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

fn capture_loop(source: NativeVideoSource, stop: Arc<AtomicBool>, done: Arc<AtomicBool>, ready_tx: oneshot::Sender<Result<(), String>>) {
    let ready: Ready = Arc::new(Mutex::new(Some(ready_tx)));
    let signal = |ready: &Ready, result: Result<(), String>| {
        if let Some(tx) = ready.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    };
    // Só o Wayland escolhe a fonte no seletor do sistema; nos demais, a lista do capturador.
    #[cfg(target_os = "linux")]
    let picker = std::env::var_os("WAYLAND_DISPLAY").is_some();
    #[cfg(not(target_os = "linux"))]
    let picker = false;
    #[cfg(target_os = "linux")]
    let source_type = if picker { DesktopCaptureSourceType::Generic } else { DesktopCaptureSourceType::Screen };
    #[cfg(not(target_os = "linux"))]
    let source_type = DesktopCaptureSourceType::Screen;
    let mut options = DesktopCapturerOptions::new(source_type);
    options.set_include_cursor(true);
    let Some(mut capturer) = DesktopCapturer::new(options) else {
        done.store(true, Ordering::Relaxed);
        signal(&ready, Err("Captura de tela indisponível neste sistema.".into()));
        return;
    };
    let selected = if picker { None } else { capturer.get_source_list().into_iter().next() };
    let callback = {
        let (ready, done) = (ready.clone(), done.clone());
        let started = Instant::now();
        move |result: Result<DesktopFrame, CaptureError>| match result {
            Ok(frame) => {
                if let Some(buffer) = to_i420(&frame) {
                    let timestamp_us = started.elapsed().as_micros() as i64;
                    let frame = VideoFrame { rotation: VideoRotation::VideoRotation0, timestamp_us, frame_metadata: None, buffer };
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
    capturer.start_capture(selected, callback);
    let interval = Duration::from_millis(1000 / FPS);
    while !stop.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
        let tick = Instant::now();
        capturer.capture_frame();
        std::thread::sleep(interval.saturating_sub(tick.elapsed()));
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
    yuv_helper::argb_to_i420(frame.data(), frame.stride(), y, stride_y, u, stride_u, v, stride_v, width, height);
    Some(buffer)
}
