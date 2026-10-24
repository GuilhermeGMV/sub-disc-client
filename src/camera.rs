//! Native camera capture runs in a child process so a stuck driver can always
//! be stopped without blocking the UI or keeping the webcam open after Leave.
use crate::bridge::{Device, Frames, RgbaFrame, UiSink};
use crate::screen::Published;
use livekit::webrtc::{
    native::yuv_helper,
    video_frame::{I420Buffer, VideoFrame, VideoRotation},
    video_source::{RtcVideoSource, VideoResolution, native::NativeVideoSource},
};
use livekit::{
    options::{TrackPublishOptions, VideoCodec, VideoEncoding},
    prelude::*,
};
use nokhwa::{
    Camera,
    pixel_format::{FormatDecoder, RgbAFormat},
    utils::{ApiBackend, CameraFormat, CameraIndex, RequestedFormat, RequestedFormatType},
};
use std::{
    io::Write,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::oneshot,
    task::JoinHandle,
};

fn device(info: &nokhwa::utils::CameraInfo) -> Device {
    Device {
        id: if !info.misc().is_empty() {
            format!("native:{}", info.misc())
        } else {
            match info.index() {
                CameraIndex::Index(i) => format!("index:{i}"),
                CameraIndex::String(s) => format!("id:{s}"),
            }
        },
        name: info.human_name(),
    }
}

pub fn list_devices() -> Result<Vec<Device>, String> {
    nokhwa::query(ApiBackend::Auto)
        .map(|items| items.iter().map(device).collect())
        .map_err(|e| format!("Não foi possível listar câmeras: {e}"))
}

fn child_command(mode: &str) -> Result<tokio::process::Command, String> {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg(mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    Ok(command)
}

pub async fn enumerate() -> Result<Vec<Device>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        child_command("--list-cameras")?.output(),
    )
    .await
    .map_err(|_| "A busca de câmeras não respondeu a tempo.")?
    .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Não foi possível listar câmeras. Verifique as permissões do sistema.".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

fn choose_format(formats: Vec<CameraFormat>) -> Option<CameraFormat> {
    formats
        .into_iter()
        .filter(|f| {
            RgbAFormat::FORMATS.contains(&f.format())
                && frame_len(f.resolution().width(), f.resolution().height()).is_ok()
                && f.frame_rate() > 0
        })
        .min_by_key(|f| {
            (
                f.resolution().width().abs_diff(1280) + f.resolution().height().abs_diff(720),
                f.frame_rate().abs_diff(30),
            )
        })
}

/// Internal subprocess entrypoint; called only after an explicit StartCamera.
pub fn worker(encoded: &str) -> Result<(), String> {
    let selected: Device = serde_json::from_str(encoded).map_err(|e| e.to_string())?;
    let devices = nokhwa::query(ApiBackend::Auto).map_err(|e| e.to_string())?;
    let info = devices
        .iter()
        .find(|info| device(info) == selected)
        .ok_or("Câmera removida ou alterada; atualize a lista.")?;
    let index = info.index().clone();
    #[cfg(windows)]
    let index = if !info.misc().is_empty() {
        CameraIndex::String(info.misc())
    } else {
        index
    };
    let mut camera = Camera::new(
        index,
        RequestedFormat::new::<RgbAFormat>(RequestedFormatType::None),
    )
    .map_err(|e| e.to_string())?;
    let format = choose_format(
        camera
            .compatible_camera_formats()
            .map_err(|e| e.to_string())?,
    )
    .ok_or("Câmera sem formato compatível.")?;
    camera
        .set_camera_requset(RequestedFormat::new::<RgbAFormat>(
            RequestedFormatType::Exact(format),
        ))
        .map_err(|e| e.to_string())?;
    camera.open_stream().map_err(|e| e.to_string())?;
    let mut output = std::io::stdout().lock();
    loop {
        let tick = Instant::now();
        let image = camera
            .frame()
            .map_err(|e| e.to_string())?
            .decode_image::<RgbAFormat>()
            .map_err(|e| e.to_string())?;
        let (width, height) = image.dimensions();
        frame_len(width, height)?;
        output
            .write_all(&width.to_le_bytes())
            .and_then(|_| output.write_all(&height.to_le_bytes()))
            .and_then(|_| output.write_all(image.as_raw()))
            .and_then(|_| output.flush())
            .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(33).saturating_sub(tick.elapsed()));
    }
}

fn frame_len(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 || width > 3840 || height > 2160 {
        return Err("Resolução da câmera inválida ou acima de 4K.".into());
    }
    Ok(width as usize * height as usize * 4)
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<(u32, u32, Vec<u8>), String> {
    let width = reader.read_u32_le().await.map_err(|e| e.to_string())?;
    let height = reader.read_u32_le().await.map_err(|e| e.to_string())?;
    let mut data = vec![0; frame_len(width, height)?];
    reader
        .read_exact(&mut data)
        .await
        .map_err(|e| e.to_string())?;
    Ok((width, height, data))
}

fn deliver(
    source: &NativeVideoSource,
    frames: &Frames,
    key: &str,
    image: (u32, u32, Vec<u8>),
    seq: u64,
    started: Instant,
) {
    let (width, height, data) = image;
    let mut buffer = I420Buffer::new(width, height);
    let (sy, su, sv) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    yuv_helper::abgr_to_i420(
        &data,
        width * 4,
        y,
        sy,
        u,
        su,
        v,
        sv,
        width as i32,
        height as i32,
    );
    source.capture_frame(&VideoFrame {
        rotation: VideoRotation::VideoRotation0,
        timestamp_us: started.elapsed().as_micros() as i64,
        frame_metadata: None,
        buffer,
    });
    frames.lock().unwrap().insert(
        key.into(),
        RgbaFrame {
            width: width as usize,
            height: height as usize,
            data,
            seq,
        },
    );
}

pub struct CameraShare {
    task: JoinHandle<()>,
    stop: Option<oneshot::Sender<()>>,
    publication: Option<Published>,
    frames: Frames,
    pub key: String,
}
impl CameraShare {
    pub async fn start(
        local: LocalParticipant,
        selected: Device,
        frames: Frames,
        ui: UiSink,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Self, String> {
        let mut command = child_command("--camera-worker")?;
        command.arg(serde_json::to_string(&selected).map_err(|e| e.to_string())?);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut output = child.stdout.take().ok_or("Falha ao abrir a captura.")?;
        let first = tokio::select! {
            _ = &mut cancel => return Err("Câmera cancelada.".into()),
            result = tokio::time::timeout(Duration::from_secs(15), read_frame(&mut output)) =>
                result.map_err(|_| "A câmera não respondeu a tempo.")?
                    .map_err(|_| "Não foi possível abrir a câmera. Verifique permissões, conexão e se outro app está usando o dispositivo.")?,
        };
        let source = NativeVideoSource::new(
            VideoResolution {
                width: first.0,
                height: first.1,
            },
            false,
        );
        let track = LocalTrack::Video(LocalVideoTrack::create_video_track(
            "camera",
            RtcVideoSource::Native(source.clone()),
        ));
        let publication = local
            .publish_track(
                track.clone(),
                TrackPublishOptions {
                    source: TrackSource::Camera,
                    video_codec: VideoCodec::VP8,
                    video_encoding: Some(VideoEncoding {
                        max_bitrate: 1_500_000,
                        max_framerate: 30.0,
                    }),
                    simulcast: false,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| format!("Falha ao publicar câmera: {e}"))?;
        let sid = publication.sid();
        let published = Published::new(local, &track, sid.clone());
        if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
            published.unpublish().await;
            return Err("Câmera cancelada.".into());
        }
        let key = format!("local-camera-{sid}");
        let task_frames = frames.clone();
        let task_key = key.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let started = Instant::now();
            deliver(&source, &task_frames, &task_key, first, 1, started);
            let mut seq = 1;
            loop {
                let next = tokio::select! {
                    _ = &mut stopped => break,
                    next = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut output)) => next,
                };
                match next {
                    Ok(Ok(image)) => {
                        seq += 1;
                        deliver(&source, &task_frames, &task_key, image, seq, started);
                        ui.repaint();
                    }
                    _ => break,
                }
            }
            let _ = child.kill().await;
        });
        Ok(Self {
            task,
            stop: Some(stop),
            publication: Some(published),
            frames,
            key,
        })
    }
    pub fn finished(&self) -> bool {
        self.task.is_finished()
    }
    pub async fn close(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
        if let Some(publication) = self.publication.take() {
            publication.unpublish().await;
        }
        self.frames.lock().unwrap().remove(&self.key);
    }
}
impl Drop for CameraShare {
    fn drop(&mut self) {
        self.task.abort();
        self.frames.lock().unwrap().remove(&self.key);
        if let Some(publication) = self.publication.take() {
            publication.unpublish_later();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nokhwa::utils::{CameraInfo, FrameFormat, Resolution};
    #[test]
    fn camera_selection_uses_native_identity_and_supported_resolution() {
        let a = CameraInfo::new("Webcam", "Camera", "usb-a", CameraIndex::Index(0));
        let b = CameraInfo::new("Webcam", "Camera", "usb-b", CameraIndex::Index(0));
        assert_ne!(device(&a), device(&b));
        let vga = CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30);
        let hd = CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 30);
        let huge = CameraFormat::new(Resolution::new(8000, 8000), FrameFormat::MJPEG, 30);
        assert_eq!(choose_format(vec![vga, huge]), Some(vga));
        assert_eq!(choose_format(vec![vga, hd]), Some(hd));
        assert_eq!(choose_format(vec![huge]), None);
    }
    #[tokio::test]
    async fn frame_protocol_rejects_invalid_and_truncated_frames() {
        assert!(frame_len(0, 720).is_err());
        assert!(frame_len(u32::MAX, 1).is_err());
        let mut valid = Vec::new();
        valid.extend_from_slice(&1u32.to_le_bytes());
        valid.extend_from_slice(&1u32.to_le_bytes());
        valid.extend_from_slice(&[255, 0, 0, 255]);
        assert_eq!(
            read_frame(&mut valid.as_slice()).await.unwrap().2,
            [255, 0, 0, 255]
        );
        assert!(read_frame(&mut &valid[..10]).await.is_err());
    }
}
