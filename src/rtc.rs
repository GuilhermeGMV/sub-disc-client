//! GStreamer WebRTC transport shared by the desktop and the media integration probe.
use crate::{
    api::Api,
    bridge::{Frames, RgbaFrame, UiSink},
    platform_audio::PlatformAudio,
};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_webrtc as gst_webrtc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Microphone,
    Camera,
    ScreenShare,
    ScreenShareAudio,
}
impl Source {
    pub fn audio(self) -> bool {
        matches!(self, Self::Microphone | Self::ScreenShareAudio)
    }
}
#[derive(Clone, Deserialize)]
pub struct Publication {
    pub id: String,
    pub identity: String,
    pub source: Source,
}
#[derive(Clone, Deserialize)]
pub struct Peer {
    pub identity: String,
    pub name: String,
    pub muted: bool,
}
#[derive(Clone, Deserialize)]
pub struct Snapshot {
    pub identity: String,
    pub peers: Vec<Peer>,
    pub publications: Vec<Publication>,
}
#[derive(Deserialize)]
struct Answer {
    id: String,
    sdp: String,
}

pub struct Rtc {
    pub pipeline: gst::Pipeline,
    bin: gst::Bin,
    mixer_pad: Option<(gst::Element, gst::Pad)>,
    pub webrtc: gst::Element,
    pub id: Option<String>,
    pub source: Source,
    pub volume: Option<gst::Element>,
    pub frames: Arc<AtomicU64>,
    pub samples: Arc<AtomicU64>,
    created: Instant,
    last_connected: std::sync::Mutex<Instant>,
    pub error: Option<String>,
    pub speaking: bool,
    speaking_flag: Arc<AtomicBool>,
}
impl Drop for Rtc {
    fn drop(&mut self) {
        let _ = self.bin.set_state(gst::State::Null);
        if let Some((mixer, pad)) = self.mixer_pad.take() {
            mixer.release_request_pad(&pad);
        }
        let _ = self.pipeline.remove(&self.bin);
        if self.pipeline.children().is_empty() {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }
}
impl Rtc {
    pub fn publisher(
        source: Source,
        capture: &str,
        audio: Option<&PlatformAudio>,
        test: bool,
    ) -> Result<Self, String> {
        init()?;
        let media = if source.audio() {
            let processing = if !test && source == Source::Microphone {
                let a = audio.ok_or("Áudio indisponível")?.settings();
                if (a.echo || a.agc || a.noise) && gst::ElementFactory::find("webrtcdsp").is_none()
                {
                    return Err(format!(
                        "Plugin GStreamer webrtcdsp ausente (processamento de voz). {}",
                        crate::portable::repair_hint()
                    ));
                }
                if gst::ElementFactory::find("webrtcdsp").is_some() {
                    format!(
                        "webrtcdsp name=dsp echo-cancel={} gain-control={} noise-suppression={} probe=echo !",
                        a.echo, a.agc, a.noise
                    )
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            format!(
                "audioconvert name=convert ! audioresample ! audio/x-raw,rate=48000,channels=1,format=S16LE ! {processing} volume name=micvolume ! identity name=meter ! opusenc bitrate=96000 inband-fec=true dtx=true ! rtpopuspay pt=111 ! application/x-rtp,media=audio,encoding-name=OPUS,payload=111 ! rtc."
            )
        } else {
            "videoconvert ! videoscale add-borders=true ! video/x-raw,width=(int)[1,1920],height=(int)[1,1080],pixel-aspect-ratio=1/1 ! videorate drop-only=true ! video/x-raw,framerate=15/1 ! queue max-size-buffers=2 max-size-time=0 max-size-bytes=0 leaky=downstream ! vp8enc deadline=1 cpu-used=8 target-bitrate=2500000 keyframe-max-dist=45 threads=2 error-resilient=partitions ! rtpvp8pay pt=96 picture-id-mode=15-bit ! application/x-rtp,media=video,encoding-name=VP8,payload=96 ! rtc.".into()
        };
        let ssrc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            .wrapping_add(1);
        let media = media
            .replace(
                "rtpopuspay pt=111",
                &format!("rtpopuspay pt=111 ssrc={ssrc}"),
            )
            .replace("rtpvp8pay pt=96", &format!("rtpvp8pay pt=96 ssrc={ssrc}"))
            .replace("payload=111 !", &format!("payload=111,ssrc=(uint){ssrc} !"))
            .replace("payload=96 !", &format!("payload=96,ssrc=(uint){ssrc} !"));
        let prefix = if source == Source::Microphone && !test {
            String::new()
        } else {
            format!("{capture} ! ")
        };
        let bin = gst::parse::bin_from_description(
            &format!("webrtcbin name=rtc bundle-policy=max-bundle latency=50 {prefix}{media}"),
            false,
        )
        .map_err(|e| e.to_string())?;
        if source == Source::Microphone && !test {
            let source = audio.unwrap().element(true)?;
            bin.add(&source).map_err(|e| e.to_string())?;
            source
                .link(&bin.by_name("convert").unwrap())
                .map_err(|e| e.to_string())?;
        }
        let rtc = Self::new(
            bin,
            source,
            if source == Source::Microphone && !test {
                audio
            } else {
                None
            },
        )?;
        if let Some(meter) = rtc.bin.by_name("meter") {
            attach_meter(&meter, &rtc.speaking_flag);
        }
        Ok(rtc)
    }
    fn new(bin: gst::Bin, source: Source, audio: Option<&PlatformAudio>) -> Result<Self, String> {
        let pipeline = if let Some(audio) = audio {
            audio.playback()?.0
        } else {
            gst::Pipeline::new()
        };
        pipeline.add(&bin).map_err(|e| e.to_string())?;
        let webrtc = bin.by_name("rtc").ok_or("webrtcbin ausente")?;
        Ok(Self {
            pipeline,
            bin,
            mixer_pad: None,
            webrtc,
            id: None,
            source,
            volume: None,
            frames: Arc::new(AtomicU64::new(0)),
            samples: Arc::new(AtomicU64::new(0)),
            created: Instant::now(),
            last_connected: std::sync::Mutex::new(Instant::now()),
            error: None,
            speaking: false,
            speaking_flag: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn receiver(
        source: Source,
        audio: &PlatformAudio,
        key: String,
        frames: Frames,
        ui: Option<UiSink>,
        test: bool,
    ) -> Result<Self, String> {
        init()?;
        let root = gst::Bin::new();
        let webrtc = gst::ElementFactory::make("webrtcbin")
            .name("rtc")
            .property_from_str("bundle-policy", "max-bundle")
            .property("latency", 50u32)
            .build()
            .map_err(|e| e.to_string())?;
        root.add(&webrtc).map_err(|e| e.to_string())?;
        let mut rtc = Self::new(
            root,
            source,
            if source.audio() && !test {
                Some(audio)
            } else {
                None
            },
        )?;
        let caps = gst::Caps::builder("application/x-rtp")
            .field("media", if source.audio() { "audio" } else { "video" })
            .field("encoding-name", if source.audio() { "OPUS" } else { "VP8" })
            .field(
                "clock-rate",
                if source.audio() { 48000i32 } else { 90000i32 },
            )
            .field("payload", if source.audio() { 111i32 } else { 96i32 })
            .build();
        let transceiver = rtc.webrtc.emit_by_name::<gst_webrtc::WebRTCRTPTransceiver>(
            "add-transceiver",
            &[&gst_webrtc::WebRTCRTPTransceiverDirection::Recvonly, &caps],
        );
        transceiver.set_property("do-nack", true);
        let bin = if source.audio() {
            let bin=gst::parse::bin_from_description("queue max-size-buffers=32 max-size-time=100000000 max-size-bytes=0 leaky=downstream ! rtpopusdepay ! opusdec plc=true use-inband-fec=true ! audioconvert ! audioresample ! audio/x-raw,rate=48000,format=S16LE ! identity name=meter ! volume name=volume ! identity name=counter ! fakesink name=placeholder",true).map_err(|e|e.to_string())?;
            rtc.volume = bin.by_name("volume");
            bin.remove(&bin.by_name("placeholder").unwrap())
                .map_err(|e| e.to_string())?;
            if test {
                let sink = gst::ElementFactory::make("fakesink")
                    .property("sync", false)
                    .build()
                    .map_err(|e| e.to_string())?;
                bin.add(&sink).map_err(|e| e.to_string())?;
                bin.by_name("counter")
                    .unwrap()
                    .link(&sink)
                    .map_err(|e| e.to_string())?;
            } else {
                let pad = bin.by_name("counter").unwrap().static_pad("src").unwrap();
                bin.add_pad(
                    &gst::GhostPad::builder_with_target(&pad)
                        .map_err(|e| e.to_string())?
                        .name("src")
                        .build(),
                )
                .map_err(|e| e.to_string())?;
            }
            let samples = rtc.samples.clone();
            bin.by_name("counter")
                .unwrap()
                .static_pad("src")
                .unwrap()
                .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    samples.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            bin
        } else {
            // RTP is never dropped before depayloading: losing part of a keyframe
            // burst stalls decoding until the next one. Only decoded frames are
            // dropped, so a slow conversion keeps just the newest frame.
            let bin=gst::parse::bin_from_description("queue max-size-buffers=0 max-size-time=200000000 max-size-bytes=0 ! rtpvp8depay request-keyframe=true wait-for-keyframe=true ! vp8dec ! queue max-size-buffers=1 max-size-time=0 max-size-bytes=0 leaky=downstream ! videoconvert ! video/x-raw,format=RGBA ! appsink name=frames sync=false max-buffers=1 drop=true",true).map_err(|e|e.to_string())?;
            let sink = bin
                .by_name("frames")
                .unwrap()
                .downcast::<gst_app::AppSink>()
                .unwrap();
            let counter = rtc.frames.clone();
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let info = gstreamer_video::VideoInfo::from_caps(
                            sample.caps().ok_or(gst::FlowError::Error)?,
                        )
                        .map_err(|_| gst::FlowError::Error)?;
                        let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                        let frame =
                            gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                                .map_err(|_| gst::FlowError::Error)?;
                        let (width, height) = (info.width() as usize, info.height() as usize);
                        let stride = info.stride()[0] as usize;
                        let data = frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
                        let mut frames = frames.lock().unwrap();
                        let entry = frames.entry(key.clone()).or_insert_with(|| RgbaFrame {
                            width,
                            height,
                            data: vec![],
                            seq: 0,
                        });
                        entry.width = width;
                        entry.height = height;
                        entry.data.resize(width * height * 4, 0);
                        entry.seq += 1;
                        for y in 0..height {
                            entry.data[y * width * 4..(y + 1) * width * 4]
                                .copy_from_slice(&data[y * stride..y * stride + width * 4]);
                        }
                        drop(frames);
                        counter.fetch_add(1, Ordering::Relaxed);
                        if let Some(ui) = &ui {
                            ui.repaint();
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
            bin
        };
        if let Some(meter) = bin.by_name("meter") {
            attach_meter(&meter, &rtc.speaking_flag);
        }
        rtc.bin.add(&bin).map_err(|e| e.to_string())?;
        if source.audio() && !test {
            let output = bin.static_pad("src").unwrap();
            let ghost = gst::GhostPad::builder_with_target(&output)
                .map_err(|e| e.to_string())?
                .name("src")
                .build();
            rtc.bin.add_pad(&ghost).map_err(|e| e.to_string())?;
            let (_, mixer) = audio.playback()?;
            let sink = mixer
                .request_pad_simple("sink_%u")
                .ok_or("Mixer indisponível")?;
            ghost.link(&sink).map_err(|e| e.to_string())?;
            rtc.mixer_pad = Some((mixer, sink));
        }
        let weak = bin.downgrade();
        rtc.webrtc.connect_pad_added(move |_, pad| {
            if pad.direction() != gst::PadDirection::Src {
                return;
            }
            if let Some(bin) = weak.upgrade()
                && let Some(sink) = bin.static_pad("sink")
                && !sink.is_linked()
                && let Err(e) = pad.link(&sink)
            {
                log::warn!("link RTP: {e}");
            }
        });
        Ok(rtc)
    }
    pub async fn negotiate(
        &mut self,
        api: &Api,
        lease: &str,
        ice: &Value,
        publication: Option<&str>,
    ) -> Result<(), String> {
        self.configure_ice(ice)?;
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| e.to_string())?;
        self.bin
            .sync_state_with_parent()
            .map_err(|e| e.to_string())?;
        let (promise, future) = gst::Promise::new_future();
        self.webrtc
            .emit_by_name::<()>("create-offer", &[&None::<gst::Structure>, &promise]);
        let reply = tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .map_err(|_| "SDP timeout")?
            .map_err(|e| format!("{e:?}"))?;
        let offer = reply
            .ok_or("SDP vazio")?
            .get::<gst_webrtc::WebRTCSessionDescription>("offer")
            .map_err(|e| e.to_string())?;
        set_description(&self.webrtc, "set-local-description", &offer).await?;
        let deadline = Instant::now() + Duration::from_secs(12);
        while self
            .webrtc
            .property::<gst_webrtc::WebRTCICEGatheringState>("ice-gathering-state")
            != gst_webrtc::WebRTCICEGatheringState::Complete
        {
            if Instant::now() > deadline {
                return Err("ICE timeout".into());
            }
            self.poll();
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let description = self
            .webrtc
            .property::<Option<gst_webrtc::WebRTCSessionDescription>>("local-description")
            .ok_or("SDP local ausente")?;
        let sdp = description.sdp().as_text().map_err(|e| e.to_string())?;
        if std::env::var_os("SUBDISC_RTC_DEBUG").is_some() {
            eprintln!("offer {:?}: {sdp}", self.source);
        }
        let body = if let Some(publication) = publication {
            json!({"lease":lease,"publication":publication,"sdp":sdp})
        } else {
            json!({"lease":lease,"source":self.source,"sdp":sdp})
        };
        let answer: Answer = api
            .request(
                reqwest::Method::POST,
                if publication.is_some() {
                    "/media/subscribe"
                } else {
                    "/media/publish"
                },
                Some(body),
            )
            .await
            .map_err(|e| e.to_string())?;
        if std::env::var_os("SUBDISC_RTC_DEBUG").is_some() {
            eprintln!("answer {:?}: {}", self.source, answer.sdp);
        }
        self.id = Some(answer.id);
        let sdp = gstreamer_sdp::SDPMessage::parse_buffer(answer.sdp.as_bytes())
            .map_err(|e| e.to_string())?;
        set_description(
            &self.webrtc,
            "set-remote-description",
            &gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Answer, sdp),
        )
        .await?;
        Ok(())
    }
    fn configure_ice(&self, ice: &Value) -> Result<(), String> {
        let Some(servers) = ice.as_array() else {
            return Ok(());
        };
        for server in servers {
            let urls = if let Some(url) = server["urls"].as_str() {
                vec![url.to_owned()]
            } else {
                server["urls"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for url in urls {
                if let Some(host) = url.strip_prefix("stun:") {
                    self.webrtc.set_property(
                        "stun-server",
                        format!("stun://{}", host.trim_start_matches('/')),
                    );
                } else if url.starts_with("turn:") || url.starts_with("turns:") {
                    let (scheme, host) = url.split_once(':').unwrap();
                    let username = server["username"].as_str().unwrap_or("");
                    let password = server["credential"].as_str().unwrap_or("");
                    let mut parsed = reqwest::Url::parse(&format!(
                        "{scheme}://{}",
                        host.trim_start_matches('/')
                    ))
                    .map_err(|e| e.to_string())?;
                    parsed.set_username(username).map_err(|_| "TURN username")?;
                    parsed
                        .set_password(Some(password))
                        .map_err(|_| "TURN credential")?;
                    if !self
                        .webrtc
                        .emit_by_name::<bool>("add-turn-server", &[&parsed.as_str()])
                    {
                        return Err("TURN URI inválida".into());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn set_muted(&self, muted: bool) {
        if let Some(volume) = self.bin.by_name("micvolume") {
            volume.set_property("mute", muted);
        }
    }
    pub fn poll(&mut self) {
        self.speaking = self.speaking_flag.load(Ordering::Relaxed);
        if let Some(bus) = self.pipeline.bus() {
            while let Some(message) = bus.pop() {
                match message.view() {
                    gst::MessageView::Error(e) => {
                        self.error = Some(format!(
                            "{}: {}",
                            e.src()
                                .map(|v| v.path_string().to_string())
                                .unwrap_or_default(),
                            e.error()
                        ));
                    }
                    gst::MessageView::Eos(_) => self.error = Some("Captura encerrada".into()),
                    _ => {}
                }
            }
        }
        if self
            .webrtc
            .property::<gst_webrtc::WebRTCPeerConnectionState>("connection-state")
            == gst_webrtc::WebRTCPeerConnectionState::Connected
        {
            *self.last_connected.lock().unwrap() = Instant::now();
        }
    }
    pub fn failed(&self) -> bool {
        self.error.is_some()
            || matches!(
                self.webrtc
                    .property::<gst_webrtc::WebRTCPeerConnectionState>("connection-state"),
                gst_webrtc::WebRTCPeerConnectionState::Failed
                    | gst_webrtc::WebRTCPeerConnectionState::Closed
            )
            || (self.created.elapsed() > Duration::from_secs(25)
                && self.last_connected.lock().unwrap().elapsed() > Duration::from_secs(25))
    }
    pub async fn close(mut self, api: &Api, lease: &str) {
        let _ = self.bin.set_state(gst::State::Null);
        if let Some(id) = self.id.take() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                api.request::<Value>(
                    reqwest::Method::POST,
                    "/media/close",
                    Some(json!({"lease":lease,"id":id})),
                ),
            )
            .await;
        }
    }
}
/// Initializes GStreamer once and names missing plugins instead of failing later
/// with an opaque pad-link error (for example, without libnice).
fn init() -> Result<(), String> {
    static READY: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    READY
        .get_or_init(|| {
            gst::init().map_err(|e| e.to_string())?;
            let missing: Vec<_> = [
                "webrtcbin",
                "nicesrc",
                "nicesink",
                "dtlssrtpenc",
                "dtlssrtpdec",
                "opusenc",
                "opusdec",
                "rtpopuspay",
                "rtpopusdepay",
                "vp8enc",
                "vp8dec",
                "rtpvp8pay",
                "rtpvp8depay",
            ]
            .into_iter()
            .filter(|name| gst::ElementFactory::find(name).is_none())
            .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "Plugins GStreamer ausentes: {}. {}",
                    missing.join(", "),
                    crate::portable::repair_hint()
                ))
            }
        })
        .clone()
}
fn attach_meter(meter: &gst::Element, flag: &Arc<AtomicBool>) {
    let flag = flag.clone();
    meter
        .static_pad("src")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer()
                && let Ok(data) = buffer.map_readable()
            {
                let count = data.len() / 2;
                let amplitude: u64 = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() as u64)
                    .sum();
                flag.store(
                    count > 0 && amplitude / (count as u64) > 300,
                    Ordering::Relaxed,
                );
            }
            gst::PadProbeReturn::Ok
        });
}
async fn set_description(
    rtc: &gst::Element,
    method: &str,
    description: &gst_webrtc::WebRTCSessionDescription,
) -> Result<(), String> {
    let (promise, future) = gst::Promise::new_future();
    rtc.emit_by_name::<()>(method, &[description, &promise]);
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| "SDP timeout")?
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_audio_shared_echo_pipeline_and_cleanup() {
        // Real GStreamer DSP and native object destruction; no physical device opens.
        for _ in 0..4 {
            let audio = PlatformAudio::synthetic();
            let mut mic = Rtc::publisher(Source::Microphone, "", Some(&audio), false).unwrap();
            let pipeline = mic.pipeline.downgrade();
            let bin = mic.bin.downgrade();
            let dsp = mic.bin.by_name("dsp").unwrap();
            let weak_dsp = dsp.downgrade();
            assert!(mic.pipeline.by_name("echo").is_some());
            assert!(dsp.property::<bool>("echo-cancel"));
            mic.pipeline.set_state(gst::State::Playing).unwrap();
            mic.bin.sync_state_with_parent().unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            mic.poll();
            assert!(mic.error.is_none(), "{:?}", mic.error);
            mic.set_muted(true);
            assert!(
                mic.bin
                    .by_name("micvolume")
                    .unwrap()
                    .property::<bool>("mute")
            );
            let receiver = Rtc::receiver(
                Source::ScreenShareAudio,
                &audio,
                "screen".into(),
                Frames::default(),
                None,
                false,
            )
            .unwrap();
            assert_eq!(receiver.pipeline, mic.pipeline);
            drop(receiver);
            drop(dsp);
            drop(mic);
            audio.shutdown();
            assert!(bin.upgrade().is_none());
            assert!(weak_dsp.upgrade().is_none());
            assert!(pipeline.upgrade().is_none());
        }
    }
}
