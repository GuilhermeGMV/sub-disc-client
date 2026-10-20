//! Native devices and voice processing through GStreamer, independent of signaling.
use gstreamer::{self as gst, prelude::*};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
pub struct PlatformAudio(Arc<Mutex<Settings>>, Arc<Mutex<Option<Output>>>);
struct Output {
    pipeline: gst::Pipeline,
    mixer: gst::Element,
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
#[derive(Clone)]
pub struct Settings {
    pub input: String,
    pub output: String,
    pub echo: bool,
    pub agc: bool,
    pub noise: bool,
    pub revision: u64,
    #[cfg(test)]
    pub synthetic: bool,
}
#[derive(Clone)]
pub struct DeviceId(String);
pub type RecordingDeviceId = DeviceId;
pub type PlayoutDeviceId = DeviceId;
impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn from_unchecked_guid(id: &str) -> Self {
        Self(id.into())
    }
}
pub struct NativeDevice {
    pub id: DeviceId,
    pub name: String,
}
impl PlatformAudio {
    pub fn new() -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        Ok(Self(
            Arc::new(Mutex::new(Settings {
                input: "default".into(),
                output: "default".into(),
                echo: true,
                agc: true,
                noise: true,
                revision: 0,
                #[cfg(test)]
                synthetic: false,
            })),
            Arc::new(Mutex::new(None)),
        ))
    }
    #[cfg(test)]
    pub fn synthetic() -> Self {
        let audio = Self::new().unwrap();
        audio.0.lock().unwrap().synthetic = true;
        audio
    }
    pub fn settings(&self) -> Settings {
        self.0.lock().unwrap().clone()
    }
    pub fn recording_devices(&self) -> std::vec::IntoIter<NativeDevice> {
        self.devices(true).into_iter()
    }
    pub fn playout_devices(&self) -> std::vec::IntoIter<NativeDevice> {
        self.devices(false).into_iter()
    }
    fn devices(&self, input: bool) -> Vec<NativeDevice> {
        let mut devices = vec![NativeDevice {
            id: DeviceId("default".into()),
            name: "Padrão do sistema".into(),
        }];
        for device in monitor(input) {
            let name = device.display_name().to_string();
            let key = device_key(&device);
            devices.push(NativeDevice {
                id: DeviceId(key),
                name,
            });
        }
        devices
    }
    pub fn set_recording_device(&self, id: &DeviceId) -> Result<(), String> {
        self.select(true, id)
    }
    pub fn switch_recording_device(&self, id: &DeviceId) -> Result<(), String> {
        self.select(true, id)
    }
    pub fn set_playout_device(&self, id: &DeviceId) -> Result<(), String> {
        self.select(false, id)
    }
    pub fn switch_playout_device(&self, id: &DeviceId) -> Result<(), String> {
        self.select(false, id)
    }
    fn select(&self, input: bool, id: &DeviceId) -> Result<(), String> {
        if id.0 != "default" && !monitor(input).iter().any(|d| device_key(d) == id.0) {
            return Err("Dispositivo removido".into());
        }
        let mut s = self.0.lock().unwrap();
        if input {
            s.input = id.0.clone()
        } else {
            s.output = id.0.clone()
        };
        s.revision += 1;
        Ok(())
    }
    pub fn set_echo_cancellation(&self, value: bool, _restart: bool) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.echo = value;
        s.revision += 1;
        Ok(())
    }
    pub fn set_auto_gain_control(&self, value: bool, _restart: bool) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.agc = value;
        s.revision += 1;
        Ok(())
    }
    pub fn set_noise_suppression(&self, value: bool, _restart: bool) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.noise = value;
        s.revision += 1;
        Ok(())
    }
    pub fn element(&self, input: bool) -> Result<gst::Element, String> {
        let settings = self.settings();
        #[cfg(test)]
        if settings.synthetic {
            let element =
                gst::ElementFactory::make(if input { "audiotestsrc" } else { "fakesink" })
                    .build()
                    .map_err(|e| e.to_string())?;
            if input {
                element.set_property("is-live", true);
            }
            return Ok(element);
        }
        let id = if input {
            settings.input
        } else {
            settings.output
        };
        if id != "default" {
            let device = monitor(input)
                .into_iter()
                .find(|d| device_key(d) == id)
                .ok_or("Dispositivo removido")?;
            return device
                .create_element(Some(if input { "capture" } else { "speaker" }))
                .map_err(|e| e.to_string());
        }
        gst::ElementFactory::make(if input {
            "autoaudiosrc"
        } else {
            "autoaudiosink"
        })
        .name(if input { "capture" } else { "speaker" })
        .build()
        .map_err(|e| e.to_string())
    }
    pub fn shutdown(&self) {
        self.1.lock().unwrap().take();
    }
    pub fn playback(&self) -> Result<(gst::Pipeline, gst::Element), String> {
        let mut output = self.1.lock().unwrap();
        if output.is_none() {
            let probe = if gst::ElementFactory::find("webrtcechoprobe").is_some() {
                "webrtcechoprobe name=echo ! "
            } else {
                ""
            };
            let pipeline=gst::parse::launch(&format!("audiomixer name=playback force-live=true ignore-inactive-pads=true latency=10000000 ! audioconvert ! audioresample ! audio/x-raw,rate=48000 ! {probe}audioconvert name=output")).map_err(|e|e.to_string())?.downcast::<gst::Pipeline>().map_err(|_|"Pipeline de áudio inválido")?;
            let sink = self.element(false)?;
            pipeline.add(&sink).map_err(|e| e.to_string())?;
            pipeline
                .by_name("output")
                .unwrap()
                .link(&sink)
                .map_err(|e| e.to_string())?;
            let mixer = pipeline.by_name("playback").unwrap();
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|e| e.to_string())?;
            *output = Some(Output { pipeline, mixer });
        }
        let output = output.as_ref().unwrap();
        Ok((output.pipeline.clone(), output.mixer.clone()))
    }
}
fn device_key(device: &gst::Device) -> String {
    if let Some(properties) = device.properties() {
        for field in [
            "device.strid",
            "object.path",
            "node.name",
            "device.path",
            "device.name",
            "device.id",
        ] {
            if let Ok(id) = properties.get::<String>(field) {
                return format!("{field}:{id}");
            }
            if let Ok(id) = properties.get::<u32>(field) {
                return format!("{field}:{id}");
            }
        }
    }
    device.display_name().to_string()
}
fn monitor(input: bool) -> Vec<gst::Device> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(
        Some(if input { "Audio/Source" } else { "Audio/Sink" }),
        None,
    );
    if monitor.start().is_err() {
        return vec![];
    }
    let devices = monitor.devices();
    monitor.stop();
    devices.into_iter().collect()
}
