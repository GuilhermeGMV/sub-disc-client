//! Capture only the explicitly selected monitor/window. Wayland uses the portal.
use crate::{
    api::Api,
    capture::{self, CaptureKind, ShareRequest},
    rtc::{Rtc, Source},
};
use gstreamer::prelude::*;
use serde_json::Value;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
pub struct ScreenShare {
    pub rtc: Rtc,
    _capture: Capture,
}
pub struct Capture {
    pub description: String,
    #[cfg(target_os = "linux")]
    x11: Option<capture::CaptureSourceInfo>,
    #[cfg(target_os = "linux")]
    session: Option<ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>>,
    #[cfg(target_os = "linux")]
    _fd: Option<std::os::fd::OwnedFd>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(session) = self.session.take() {
            tokio::spawn(async move {
                let _ = session.close().await;
            });
        }
    }
}
impl Capture {
    pub fn attach(&self, source: Option<gstreamer::Element>) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if let Some(selected) = &self.x11 {
            crate::x11_capture::attach(source.ok_or("Fonte X11 ausente")?, selected.clone())?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = source;
        Ok(())
    }
    pub async fn select(request: ShareRequest) -> Result<Self, String> {
        request.validate(false)?;
        let mut capture = Self {
            description: String::new(),
            #[cfg(target_os = "linux")]
            x11: None,
            #[cfg(target_os = "linux")]
            session: None,
            #[cfg(target_os = "linux")]
            _fd: None,
        };
        if request.source.kind == CaptureKind::Portal {
            #[cfg(target_os = "linux")]
            {
                use ashpd::desktop::screencast::{
                    CursorMode, Screencast, SelectSourcesOptions, SourceType,
                };
                let portal = Screencast::new().await.map_err(|e| e.to_string())?;
                let session = portal
                    .create_session(Default::default())
                    .await
                    .map_err(|e| e.to_string())?;
                capture.session = Some(session);
                let session = capture.session.as_ref().unwrap();
                portal
                    .select_sources(
                        session,
                        SelectSourcesOptions::default()
                            .set_sources(SourceType::Monitor | SourceType::Window)
                            .set_multiple(false)
                            .set_cursor_mode(CursorMode::Embedded),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .response()
                    .map_err(|e| e.to_string())?;
                let streams = portal
                    .start(session, None, Default::default())
                    .await
                    .map_err(|e| e.to_string())?
                    .response()
                    .map_err(|e| e.to_string())?;
                let node = streams
                    .streams()
                    .first()
                    .ok_or("Nenhuma tela escolhida")?
                    .pipe_wire_node_id();
                let fd = portal
                    .open_pipe_wire_remote(session, Default::default())
                    .await
                    .map_err(|e| e.to_string())?;
                capture.description = format!(
                    "pipewiresrc fd={} path={} do-timestamp=true",
                    fd.as_raw_fd(),
                    node
                );
                capture._fd = Some(fd);
                return Ok(capture);
            }
            #[cfg(not(target_os = "linux"))]
            return Err("Portal indisponível".into());
        }
        capture::selected(&capture::enumerate()?, &request.source)?;
        #[cfg(target_os = "linux")]
        {
            capture.description =
                "appsrc name=screen_source is-live=true format=time do-timestamp=true block=false"
                    .into();
            capture.x11 = Some(request.source);
        }
        #[cfg(target_os = "windows")]
        {
            capture.description = format!(
                "d3d11screencapturesrc capture-api=wgc {}={} ! d3d11download",
                if request.source.kind == CaptureKind::Window {
                    "window-handle"
                } else {
                    "monitor-handle"
                },
                request.source.id
            );
        }
        if capture.description.is_empty() {
            return Err("Captura indisponível neste sistema".into());
        }
        Ok(capture)
    }
}
impl ScreenShare {
    pub async fn start(
        api: &Api,
        lease: &str,
        ice: &Value,
        request: ShareRequest,
    ) -> Result<Self, String> {
        let capture = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            Capture::select(request),
        )
        .await
        .map_err(|_| "Tempo de seleção de tela esgotado")??;
        let mut rtc = Rtc::publisher(Source::ScreenShare, &capture.description, None, false)?;
        capture.attach(rtc.pipeline.by_name("screen_source"))?;
        rtc.negotiate(api, lease, ice, None).await?;
        Ok(Self {
            rtc,
            _capture: capture,
        })
    }
    pub async fn recover(self, api: &Api, lease: &str, ice: &Value) -> Result<Self, String> {
        let Self { rtc, _capture } = self;
        rtc.close(api, lease).await;
        let mut rtc = Rtc::publisher(Source::ScreenShare, &_capture.description, None, false)?;
        _capture.attach(rtc.pipeline.by_name("screen_source"))?;
        rtc.negotiate(api, lease, ice, None).await?;
        Ok(Self { rtc, _capture })
    }
    pub fn finished(&mut self) -> bool {
        self.rtc.poll();
        self.rtc.failed()
    }
    pub async fn close(self, api: &Api, lease: &str) {
        self.rtc.close(api, lease).await;
    }
}

#[cfg(target_os = "windows")]
pub fn windows_sources() -> Result<Vec<capture::CaptureSourceInfo>, String> {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
        },
    };
    use windows_sys::core::BOOL;
    unsafe extern "system" fn window(hwnd: HWND, data: LPARAM) -> BOOL {
        let sources = unsafe { &mut *(data as *mut Vec<capture::CaptureSourceInfo>) };
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            let len = unsafe { GetWindowTextLengthW(hwnd) };
            if len > 0 {
                let mut title = vec![0u16; len as usize + 1];
                let count = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
                sources.push(capture::CaptureSourceInfo {
                    id: hwnd as u64,
                    title: String::from_utf16_lossy(&title[..count as usize]),
                    kind: CaptureKind::Window,
                });
            }
        }
        1
    }
    unsafe extern "system" fn monitor(
        handle: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let sources = unsafe { &mut *(data as *mut Vec<capture::CaptureSourceInfo>) };
        sources.push(capture::CaptureSourceInfo {
            id: handle as u64,
            title: format!("Monitor {}", sources.len() + 1),
            kind: CaptureKind::Monitor,
        });
        1
    }
    let mut sources = vec![];
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(monitor),
            &mut sources as *mut _ as LPARAM,
        );
        EnumWindows(Some(window), &mut sources as *mut _ as LPARAM);
    }
    Ok(sources)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use gstreamer as gst;
    #[tokio::test]
    #[ignore = "requires isolated Xvfb and SUBDISC_TEST_CAPTURE=1"]
    async fn x11_sources_selection_removal_and_native_cleanup() {
        assert_eq!(std::env::var("SUBDISC_TEST_CAPTURE").as_deref(), Ok("1"));
        let mut window = std::process::Command::new("xmessage")
            .args([
                "-title",
                "Subdisc synthetic capture",
                "-geometry",
                "320x200",
                "Synthetic test content",
            ])
            .spawn()
            .unwrap();
        struct Stop(u32);
        impl Drop for Stop {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0 as i32, libc::SIGTERM);
                }
            }
        }
        let stop = Stop(window.id());
        let source = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(source) = capture::enumerate()
                    .unwrap()
                    .into_iter()
                    .find(|v| v.title == "Subdisc synthetic capture")
                {
                    break source;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let request = ShareRequest {
            source: source.clone(),
            video: true,
            audio: false,
        };
        let capture = Capture::select(request.clone()).await.unwrap();
        gst::init().unwrap();
        let pipeline = gst::parse::launch(&format!(
            "{} ! videoconvert ! appsink name=frames max-buffers=1 drop=true sync=false",
            capture.description
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        capture.attach(pipeline.by_name("screen_source")).unwrap();
        let weak = pipeline.downgrade();
        let sink = pipeline
            .by_name("frames")
            .unwrap()
            .downcast::<gstreamer_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let sample = sink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("selected window must produce pixels");
        assert!(sample.buffer().unwrap().size() > 0);
        window.kill().unwrap();
        window.wait().unwrap();
        std::mem::forget(stop);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if capture::selected(&capture::enumerate().unwrap(), &source).is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(Capture::select(request).await.is_err());
        pipeline.set_state(gst::State::Null).unwrap();
        drop(sample);
        drop(sink);
        drop(pipeline);
        drop(capture);
        assert!(weak.upgrade().is_none());
    }
}
