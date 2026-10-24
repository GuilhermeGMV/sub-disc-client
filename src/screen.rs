//! Compartilhamento de tela com o capturador do próprio libwebrtc. No Wayland ele
//! abre o seletor do portal (xdg-desktop-portal + PipeWire); no X11 e no Windows
//! captura exclusivamente o monitor ou janela confirmado pelo usuário.
//!
//! Vídeo (`Screenshare`) e áudio (`ScreenshareAudio`, ver `system_audio`) são
//! duas tracks da mesma identidade, cada uma com worker e publicação próprios.
//! O início é tudo-ou-nada: se uma parte falha, nenhuma fica transmitindo.
use crate::capture::{self, AudioAvailability, CaptureKind, CaptureWorker, ShareRequest};
use crate::system_audio;
use livekit::options::{AudioEncoding, TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::audio_source::RtcAudioSource;
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
const AUDIO_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Mesmo `stream` nas duas tracks: o receptor as agrupa e sincroniza (lip-sync).
const STREAM: &str = "screen_share";
const CANCELLED: &str = "Compartilhamento cancelado.";

pub fn video_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Screenshare,
        video_codec: VideoCodec::VP8,
        video_encoding: Some(VideoEncoding {
            max_bitrate: 2_500_000,
            max_framerate: FPS as f64,
        }),
        simulcast: false,
        stream: STREAM.into(),
        ..Default::default()
    }
}

/// Música/jogo em estéreo: sem DTX (cortaria trechos baixos), como o navegador
/// faz com `musicHighQualityStereo`.
pub fn audio_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::ScreenshareAudio,
        audio_encoding: Some(AudioEncoding {
            max_bitrate: 128_000,
        }),
        dtx: false,
        red: true,
        stream: STREAM.into(),
        ..Default::default()
    }
}

/// Publica/despublica tracks locais. Implementado pela sala real e, nos
/// testes, por um publicador falso que pode falhar sob demanda.
pub trait Publisher: Clone + Send + Sync + 'static {
    fn publish(
        &self,
        track: LocalTrack,
        options: TrackPublishOptions,
    ) -> impl Future<Output = Result<TrackSid, String>> + Send;
    fn unpublish(&self, track: String, sid: TrackSid) -> impl Future<Output = ()> + Send;
}

impl Publisher for LocalParticipant {
    async fn publish(
        &self,
        track: LocalTrack,
        options: TrackPublishOptions,
    ) -> Result<TrackSid, String> {
        self.publish_track(track, options)
            .await
            .map(|publication| publication.sid())
            .map_err(|e| e.to_string())
    }

    /// O SID muda quando o SDK republica tudo numa reconexão completa; a
    /// publicação atual é encontrada pelo ID da track local.
    async fn unpublish(&self, track: String, sid: TrackSid) {
        let sid = self
            .track_publications()
            .into_values()
            .find(|p| p.track().is_some_and(|t| t.rtc_track().id() == track))
            .map_or(sid, |p| p.sid());
        let _ = self.unpublish_track(&sid).await;
    }
}

/// Uma track local publicada, despublicada exatamente uma vez.
pub struct Published<P: Publisher = LocalParticipant> {
    publisher: P,
    track: String,
    sid: TrackSid,
}

impl<P: Publisher> Published<P> {
    pub fn new(publisher: P, track: &LocalTrack, sid: TrackSid) -> Self {
        Self {
            publisher,
            track: track.rtc_track().id(),
            sid,
        }
    }

    pub async fn unpublish(self) {
        self.publisher.unpublish(self.track, self.sid).await;
    }

    pub fn unpublish_later(self) {
        tokio::spawn(self.unpublish());
    }
}

/// Worker nativo + publicação de uma das tracks.
struct Part<P: Publisher> {
    worker: CaptureWorker,
    failure: Arc<Mutex<Option<String>>>,
    publication: Option<Published<P>>,
}

impl<P: Publisher> Part<P> {
    fn new(worker: CaptureWorker, failure: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            worker,
            failure,
            publication: None,
        }
    }

    async fn close(mut self) {
        self.worker.close().await;
        if let Some(publication) = self.publication.take() {
            publication.unpublish().await;
        }
    }
}

impl<P: Publisher> Drop for Part<P> {
    fn drop(&mut self) {
        self.worker.stop();
        if let Some(publication) = self.publication.take() {
            publication.unpublish_later();
        }
    }
}

/// O que mudou numa transmissão em andamento.
#[derive(Debug, PartialEq, Eq)]
pub enum ShareChange {
    /// Nada mais é transmitido; a sessão descarta o compartilhamento.
    Ended(String),
    /// Só o áudio parou (dispositivo/aplicativo); o vídeo continua.
    AudioStopped(String),
}

pub struct ScreenShare<P: Publisher = LocalParticipant> {
    video: Option<Part<P>>,
    audio: Option<Part<P>>,
}

impl<P: Publisher> ScreenShare<P> {
    /// Abre o seletor, espera o primeiro frame, abre o áudio e publica as tracks.
    pub async fn start(
        publisher: P,
        request: ShareRequest,
        audio: AudioAvailability,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Self, String> {
        request.validate(&audio)?;
        log::info!(
            "compartilhamento: iniciando (vídeo: {}, áudio: {:?}, fonte: {:?})",
            request.video,
            request.audio,
            request.source.as_ref().map(|s| (s.kind, s.id))
        );
        let mut share = Self {
            video: None,
            audio: None,
        };
        let result = async {
            let video = if request.video {
                Some(share.start_video(&request, &mut cancel).await?)
            } else {
                None
            };
            // O áudio só abre depois que a fonte de vídeo foi confirmada.
            let audio = match request.audio {
                Some(scope) => Some(share.start_audio(scope, &request, &mut cancel).await?),
                None => None,
            };
            share.publish(&publisher, video, audio, &mut cancel).await
        }
        .await;
        match result {
            Ok(()) => {
                log::info!("compartilhamento: transmitindo");
                Ok(share)
            }
            Err(error) => {
                log::warn!("compartilhamento: não iniciou: {error}");
                share.close().await;
                Err(error)
            }
        }
    }

    async fn start_video(
        &mut self,
        request: &ShareRequest,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<LocalTrack, String> {
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 1920,
                height: 1080,
            },
            true,
        );
        let (ready_tx, ready_rx) = oneshot::channel();
        let (capture_source, capture_request) = (source.clone(), request.clone());
        let worker = CaptureWorker::spawn(move |stop, done| {
            capture_loop(capture_source, stop, done, ready_tx, capture_request)
        })?;
        self.video = Some(Part::new(worker, Default::default()));
        match wait(ready_rx, PICK_TIMEOUT, cancel).await? {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error),
            None => return Err("O compartilhamento de tela não começou a tempo.".into()),
        }
        log::info!("compartilhamento: primeiro frame de vídeo recebido");
        Ok(LocalTrack::Video(LocalVideoTrack::create_video_track(
            "screen",
            RtcVideoSource::Native(source),
        )))
    }

    async fn start_audio(
        &mut self,
        scope: capture::AudioScope,
        request: &ShareRequest,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<LocalTrack, String> {
        let source = system_audio::native_source();
        let (ready_tx, ready_rx) = oneshot::channel();
        let failure: Arc<Mutex<Option<String>>> = Default::default();
        let (sink_source, slot) = (source.clone(), failure.clone());
        let window = request.source.as_ref().map(|s| s.id);
        let worker = CaptureWorker::spawn_named("screen-audio", move |stop, _done| {
            let mut sink = system_audio::native_sink(sink_source);
            let ready = |result: Result<(), system_audio::AudioError>| {
                let _ = ready_tx.send(result.map_err(|e| e.message()));
            };
            if let Err(error) = system_audio::run_isolated(scope, window, &stop, ready, &mut sink) {
                log::warn!("áudio da tela encerrado: {}", error.message());
                *slot.lock().unwrap() = Some(error.message());
            }
        })?;
        self.audio = Some(Part::new(worker, failure));
        match wait(ready_rx, AUDIO_START_TIMEOUT, cancel).await? {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error),
            None => return Err("A captura de áudio não iniciou a tempo.".into()),
        }
        log::info!("compartilhamento: captura de áudio pronta");
        Ok(LocalTrack::Audio(LocalAudioTrack::create_audio_track(
            "screen_audio",
            RtcAudioSource::Native(source),
        )))
    }

    /// Publica vídeo e depois áudio. Qualquer falha ou cancelamento devolve
    /// erro e quem chamou fecha tudo, inclusive o que já foi publicado.
    async fn publish(
        &mut self,
        publisher: &P,
        video: Option<LocalTrack>,
        audio: Option<LocalTrack>,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<(), String> {
        let with_video = video.is_some();
        if let Some(track) = video {
            log::info!("compartilhamento: publicando vídeo");
            let sid = publisher
                .publish(track.clone(), video_options())
                .await
                .map_err(|e| {
                    log::warn!("falha ao publicar a tela: {e}");
                    "Não foi possível publicar o vídeo da tela.".to_owned()
                })?;
            if let Some(part) = &mut self.video {
                part.publication = Some(Published::new(publisher.clone(), &track, sid));
            }
            cancelled(cancel)?;
        }
        if let Some(track) = audio {
            log::info!("compartilhamento: publicando áudio");
            let sid = publisher
                .publish(track.clone(), audio_options())
                .await
                .map_err(|e| {
                    log::warn!("falha ao publicar o áudio da tela: {e}");
                    if with_video {
                        "Não foi possível publicar o áudio do compartilhamento. O vídeo também foi interrompido; nada está sendo transmitido.".to_owned()
                    } else {
                        "Não foi possível publicar o áudio do compartilhamento.".to_owned()
                    }
                })?;
            if let Some(part) = &mut self.audio {
                part.publication = Some(Published::new(publisher.clone(), &track, sid));
            }
            cancelled(cancel)?;
        }
        Ok(())
    }

    /// Encerra as capturas (join fora do executor) e despublica as tracks.
    pub async fn close(mut self) {
        let (video, audio) = (self.video.take(), self.audio.take());
        if let Some(audio) = audio {
            audio.close().await;
        }
        if let Some(video) = video {
            video.close().await;
        }
    }

    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    /// Alguma captura terminou por conta própria (seletor cancelado, fonte
    /// encerrada, dispositivo de áudio desconectado).
    pub fn needs_attention(&self) -> bool {
        [&self.video, &self.audio]
            .into_iter()
            .flatten()
            .any(|part| part.worker.finished())
    }

    /// Remove a parte encerrada e informa o que a pessoa precisa saber. O vídeo
    /// leva o áudio junto; o áudio sozinho não derruba o vídeo.
    pub fn poll(&mut self) -> Option<ShareChange> {
        if self.video.as_ref().is_some_and(|v| v.worker.finished()) {
            let audio = if self.audio.take().is_some() {
                " O áudio do compartilhamento também foi interrompido."
            } else {
                ""
            };
            self.video.take();
            return Some(ShareChange::Ended(format!(
                "A captura foi encerrada ou a fonte ficou indisponível.{audio}"
            )));
        }
        let part = self.audio.take_if(|a| a.worker.finished())?;
        let reason = part
            .failure
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "A captura de áudio terminou.".into());
        Some(if self.video.is_some() {
            ShareChange::AudioStopped(format!(
                "{reason} O áudio do compartilhamento foi interrompido; o vídeo continua."
            ))
        } else {
            ShareChange::Ended(format!("{reason} O compartilhamento foi encerrado."))
        })
    }
}

impl<P: Publisher> Drop for ScreenShare<P> {
    fn drop(&mut self) {
        // Cada `Part` para seu worker e agenda a despublicação.
        self.audio.take();
        self.video.take();
    }
}

/// Espera o worker ficar pronto, respeitando o cancelamento. `None` = timeout.
async fn wait(
    ready: oneshot::Receiver<Result<(), String>>,
    timeout: Duration,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<Result<(), String>>, String> {
    tokio::select! {
        result = tokio::time::timeout(timeout, ready) => Ok(match result {
            Ok(Ok(result)) => Some(result),
            // Worker terminou sem sinalizar (pânico): trata como falha.
            Ok(Err(_)) => Some(Err("A captura terminou inesperadamente.".into())),
            Err(_) => None,
        }),
        _ = cancel => Err(CANCELLED.into()),
    }
}

/// Stop/troca/saída durante a publicação: a track publicada é desfeita por quem chamou.
fn cancelled(cancel: &mut oneshot::Receiver<()>) -> Result<(), String> {
    match cancel.try_recv() {
        Err(oneshot::error::TryRecvError::Empty) => Ok(()),
        _ => Err(CANCELLED.into()),
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
    let Some(requested) = request.source else {
        done.store(true, Ordering::Relaxed);
        signal(&ready, Err("Escolha uma tela ou janela.".into()));
        return;
    };
    let kind = requested.kind;
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
                capture::selected(std::slice::from_ref(&info), &requested).is_ok()
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
                .any(|s| s.id() == requested.id)
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

#[cfg(test)]
mod lifecycle_tests {
    //! Publicador falso e workers sintéticos: nenhuma sala, tela ou dispositivo.
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone, Default)]
    struct Fake {
        fail: Option<TrackSource>,
        published: Arc<Mutex<Vec<(String, TrackSource)>>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        fn sources(&self) -> Vec<TrackSource> {
            self.published
                .lock()
                .unwrap()
                .iter()
                .map(|(_, s)| *s)
                .collect()
        }
    }

    impl Publisher for Fake {
        async fn publish(
            &self,
            track: LocalTrack,
            options: TrackPublishOptions,
        ) -> Result<TrackSid, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("publish {:?}", options.source));
            if self.fail == Some(options.source) {
                return Err("recusado pelo servidor".into());
            }
            let id = track.rtc_track().id();
            self.published
                .lock()
                .unwrap()
                .push((id.clone(), options.source));
            Ok(TrackSid::try_from(format!("TR_{id}")).unwrap())
        }

        async fn unpublish(&self, track: String, _sid: TrackSid) {
            self.calls.lock().unwrap().push("unpublish".into());
            self.published
                .lock()
                .unwrap()
                .retain(|(id, _)| *id != track);
        }
    }

    /// Worker que roda até ser parado, ou termina sozinho com `failure`.
    fn part(active: &Arc<AtomicUsize>, failure: Option<&str>) -> Part<Fake> {
        let slot: Arc<Mutex<Option<String>>> = Default::default();
        let (count, report, failure) = (active.clone(), slot.clone(), failure.map(str::to_owned));
        let worker = CaptureWorker::spawn(move |stop, _| {
            count.fetch_add(1, Ordering::SeqCst);
            if let Some(failure) = failure {
                *report.lock().unwrap() = Some(failure);
            } else {
                while !stop.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_millis(5));
                }
            }
            count.fetch_sub(1, Ordering::SeqCst);
        })
        .unwrap();
        Part::new(worker, slot)
    }

    fn tracks() -> (LocalTrack, LocalTrack) {
        let video = NativeVideoSource::new(
            VideoResolution {
                width: 64,
                height: 64,
            },
            true,
        );
        (
            LocalTrack::Video(LocalVideoTrack::create_video_track(
                "screen",
                RtcVideoSource::Native(video),
            )),
            LocalTrack::Audio(LocalAudioTrack::create_audio_track(
                "screen_audio",
                RtcAudioSource::Native(system_audio::native_source()),
            )),
        )
    }

    fn share(active: &Arc<AtomicUsize>) -> ScreenShare<Fake> {
        ScreenShare {
            video: Some(part(active, None)),
            audio: Some(part(active, None)),
        }
    }

    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn video_and_audio_publish_together_and_repeated_cycles_leave_nothing() {
        let active = Arc::new(AtomicUsize::new(0));
        let fake = Fake::default();
        for _ in 0..16 {
            let mut share = share(&active);
            let (video, audio) = tracks();
            let (_tx, mut cancel) = oneshot::channel();
            share
                .publish(&fake, Some(video), Some(audio), &mut cancel)
                .await
                .unwrap();
            assert!(share.has_video() && share.has_audio());
            assert_eq!(
                fake.sources(),
                [TrackSource::Screenshare, TrackSource::ScreenshareAudio]
            );
            share.close().await;
            // Sem tracks duplicadas nem workers vivos após cada ciclo.
            assert!(fake.sources().is_empty());
            assert_eq!(active.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn audio_publish_failure_withdraws_video_and_stops_both_workers() {
        let active = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            fail: Some(TrackSource::ScreenshareAudio),
            ..Default::default()
        };
        let mut share = share(&active);
        let (video, audio) = tracks();
        let (_tx, mut cancel) = oneshot::channel();
        let error = share
            .publish(&fake, Some(video), Some(audio), &mut cancel)
            .await
            .unwrap_err();
        assert!(error.contains("nada está sendo transmitido"), "{error}");
        // `start` fecha tudo no erro: o vídeo já publicado é retirado.
        assert_eq!(fake.sources(), [TrackSource::Screenshare]);
        share.close().await;
        assert!(fake.sources().is_empty());
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_during_startup_never_publishes_late_tracks() {
        let active = Arc::new(AtomicUsize::new(0));
        let fake = Fake::default();
        let mut share = share(&active);
        let (video, audio) = tracks();
        let (tx, mut cancel) = oneshot::channel();
        tx.send(()).unwrap();
        let error = share
            .publish(&fake, Some(video), Some(audio), &mut cancel)
            .await
            .unwrap_err();
        assert_eq!(error, CANCELLED);
        share.close().await;
        assert!(fake.sources().is_empty());
        // O áudio nunca chegou a ser publicado depois do cancelamento.
        assert_eq!(
            *fake.calls.lock().unwrap(),
            ["publish Screenshare", "unpublish"]
        );
        assert_eq!(active.load(Ordering::SeqCst), 0);

        // Cancelar enquanto espera o worker (seletor aberto) também interrompe.
        let (_ready_tx, ready_rx) = oneshot::channel();
        let (tx, mut cancel) = oneshot::channel();
        tx.send(()).unwrap();
        assert_eq!(
            wait(ready_rx, Duration::from_secs(5), &mut cancel).await,
            Err(CANCELLED.into())
        );
        let (_tx, mut cancel) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
        assert_eq!(
            wait(ready_rx, Duration::from_millis(10), &mut cancel).await,
            Ok(None)
        );
        drop(ready_tx);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
        drop(ready_tx);
        assert!(matches!(
            wait(ready_rx, Duration::from_secs(1), &mut cancel).await,
            Ok(Some(Err(_)))
        ));
    }

    #[tokio::test]
    async fn audio_loss_keeps_video_and_video_loss_ends_everything() {
        let active = Arc::new(AtomicUsize::new(0));
        let fake = Fake::default();
        let mut share = ScreenShare {
            video: Some(part(&active, None)),
            audio: Some(part(
                &active,
                Some("O dispositivo de saída de áudio foi desconectado."),
            )),
        };
        let (video, audio) = tracks();
        let (_tx, mut cancel) = oneshot::channel();
        share
            .publish(&fake, Some(video), Some(audio), &mut cancel)
            .await
            .unwrap();
        until(|| share.needs_attention()).await;
        let change = share.poll().unwrap();
        assert!(matches!(&change, ShareChange::AudioStopped(m)
            if m.contains("desconectado") && m.contains("o vídeo continua")));
        assert!(share.has_video() && !share.has_audio());
        until(|| fake.sources() == [TrackSource::Screenshare]).await;
        assert_eq!(share.poll(), None);

        // Fonte de vídeo encerrada: nada continua transmitindo.
        share.video.as_ref().unwrap().worker.stop();
        until(|| share.needs_attention()).await;
        assert!(matches!(share.poll(), Some(ShareChange::Ended(_))));
        assert!(!share.has_video());
        drop(share);
        until(|| fake.sources().is_empty() && active.load(Ordering::SeqCst) == 0).await;

        // Áudio sozinho que falha encerra o compartilhamento.
        let mut share = ScreenShare::<Fake> {
            video: None,
            audio: Some(part(&active, Some("falhou"))),
        };
        until(|| share.needs_attention()).await;
        assert!(matches!(share.poll(), Some(ShareChange::Ended(m)) if m.starts_with("falhou")));
    }
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
            source: Some(selected.clone()),
            video: true,
            audio: None,
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
            source: Some(selected),
            video: true,
            audio: None,
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
