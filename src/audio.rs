//! Preferências locais, independentes de SID/lease e da captura de microfone.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioKind {
    Microphone,
    ScreenShare,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Volume {
    #[serde(deserialize_with = "deserialize_gain")]
    pub gain: f32,
    pub muted: bool,
}

fn deserialize_gain<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    f32::deserialize(deserializer).map(clamp_gain)
}

impl Default for Volume {
    fn default() -> Self {
        Self {
            gain: 1.0,
            muted: false,
        }
    }
}

pub fn clamp_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 1.5)
    } else {
        1.0
    }
}

impl Volume {
    pub fn effective(self, deafened: bool) -> f32 {
        if self.muted || deafened {
            0.0
        } else {
            clamp_gain(self.gain)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ParticipantVolume {
    pub microphone: Volume,
    pub screen_share: Volume,
}

impl ParticipantVolume {
    pub fn get(self, kind: AudioKind) -> Volume {
        match kind {
            AudioKind::Microphone => self.microphone,
            AudioKind::ScreenShare => self.screen_share,
        }
    }

    pub fn set(&mut self, kind: AudioKind, mut volume: Volume) {
        volume.gain = clamp_gain(volume.gain);
        match kind {
            AudioKind::Microphone => self.microphone = volume,
            AudioKind::ScreenShare => self.screen_share = volume,
        }
    }
}

pub type Volumes = HashMap<String, ParticipantVolume>;

pub fn user_of(identity: &str) -> &str {
    identity.split_once(':').map_or(identity, |(user, _)| user)
}

/// Rampa de ~40 ms: aplicada pelo backend a cada 10 ms, fora da UI.
pub fn ramp(current: f32, target: f32) -> f32 {
    let delta = target - current;
    if delta.abs() < 0.002 {
        target
    } else {
        current + delta * 0.5
    }
}

pub trait PlaybackTrack {
    fn set_gain(&self, gain: f32) -> bool;
}

impl PlaybackTrack for livekit::prelude::RemoteAudioTrack {
    fn set_gain(&self, gain: f32) -> bool {
        self.rtc_track().set_playback_volume(f64::from(gain))
    }
}

/// Um único caminho de playout nativo por SID; não cria sinks PCM ou saídas.
pub struct Playback<T> {
    tracks: HashMap<String, (T, String, AudioKind, f32)>,
}

impl<T: PlaybackTrack> Playback<T> {
    pub fn new() -> Self {
        Self {
            tracks: HashMap::new(),
        }
    }

    pub fn attach(
        &mut self,
        sid: String,
        track: T,
        user: String,
        kind: AudioKind,
        prefs: &Volumes,
        deafened: bool,
    ) -> bool {
        let gain = prefs
            .get(&user)
            .copied()
            .unwrap_or_default()
            .get(kind)
            .effective(deafened);
        let applied = track.set_gain(gain);
        self.tracks.insert(sid, (track, user, kind, gain));
        applied
    }

    pub fn detach(&mut self, sid: &str) {
        self.tracks.remove(sid);
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
    }

    pub fn update(&mut self, prefs: &Volumes, deafened: bool, force: bool) {
        for (track, user, kind, current) in self.tracks.values_mut() {
            let target = prefs
                .get(user)
                .copied()
                .unwrap_or_default()
                .get(*kind)
                .effective(deafened);
            let next = if force {
                target
            } else {
                ramp(*current, target)
            };
            if (force || next != *current) && track.set_gain(next) {
                *current = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_mutes_and_independence() {
        let mut users = Volumes::new();
        let a = users.entry("a".into()).or_default();
        assert_eq!(a.microphone.effective(false), 1.0);
        a.set(
            AudioKind::Microphone,
            Volume {
                gain: -1.0,
                muted: false,
            },
        );
        a.set(
            AudioKind::ScreenShare,
            Volume {
                gain: 2.0,
                muted: false,
            },
        );
        assert_eq!(a.microphone.effective(false), 0.0);
        assert_eq!(a.screen_share.effective(false), 1.5);
        assert_eq!(a.screen_share.effective(true), 0.0);
        a.screen_share.muted = true;
        assert_eq!(a.screen_share.effective(false), 0.0);
        assert_eq!(a.screen_share.gain, 1.5);
        assert_eq!(users.entry("b".into()).or_default().microphone.gain, 1.0);
        assert_eq!(clamp_gain(f32::NAN), 1.0);
    }

    #[test]
    fn preferences_survive_replaced_tracks_and_leases() {
        let mut users = Volumes::new();
        users
            .entry(user_of("u:old").into())
            .or_default()
            .microphone
            .gain = 0.8;
        let restored: Volumes =
            serde_json::from_str(&serde_json::to_string(&users).unwrap()).unwrap();
        assert_eq!(restored[user_of("u:new")].microphone.gain, 0.8);
        let mut current = 1.0;
        for _ in 0..16 {
            current = ramp(current, 0.0);
        }
        assert_eq!(current, 0.0);
        let bounded: ParticipantVolume =
            serde_json::from_str(r#"{"microphone":{"gain":-2},"screen_share":{"gain":3}}"#)
                .unwrap();
        assert_eq!(bounded.microphone.gain, 0.0);
        assert_eq!(bounded.screen_share.gain, 1.5);
    }

    #[test]
    fn playout_registry_reconnect_and_device_reset_with_mock() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Fake(Rc<Cell<f32>>);
        impl PlaybackTrack for Fake {
            fn set_gain(&self, gain: f32) -> bool {
                self.0.set(gain);
                true
            }
        }
        let mic = Rc::new(Cell::new(-1.0));
        let screen = Rc::new(Cell::new(-1.0));
        let second_user = Rc::new(Cell::new(-1.0));
        let mut prefs = Volumes::new();
        prefs.entry("a".into()).or_default().microphone.gain = 0.8;
        prefs.entry("a".into()).or_default().screen_share.gain = 0.35;
        let mut playback = Playback::new();
        playback.attach(
            "mic".into(),
            Fake(mic.clone()),
            "a".into(),
            AudioKind::Microphone,
            &prefs,
            false,
        );
        playback.attach(
            "screen".into(),
            Fake(screen.clone()),
            "a".into(),
            AudioKind::ScreenShare,
            &prefs,
            false,
        );
        playback.attach(
            "other".into(),
            Fake(second_user.clone()),
            "b".into(),
            AudioKind::Microphone,
            &prefs,
            false,
        );
        assert_eq!(
            (mic.get(), screen.get(), second_user.get()),
            (0.8, 0.35, 1.0)
        );
        playback.update(&prefs, true, true);
        assert_eq!(
            (mic.get(), screen.get(), second_user.get()),
            (0.0, 0.0, 0.0)
        );
        playback.update(&prefs, false, true);
        assert_eq!(screen.get(), 0.35);
        // Native device/channel recreation may reset gain; reapply without another sink.
        mic.set(1.0);
        playback.update(&prefs, false, true);
        assert_eq!(mic.get(), 0.8);
        let replacement = Rc::new(Cell::new(-1.0));
        playback.attach(
            "mic".into(),
            Fake(replacement.clone()),
            "a".into(),
            AudioKind::Microphone,
            &prefs,
            false,
        );
        assert_eq!(Rc::strong_count(&mic), 1); // old track released, no duplicate ownership
        assert_eq!(replacement.get(), 0.8);
        playback.detach("screen");
        assert_eq!(Rc::strong_count(&screen), 1);
        playback.clear();
        assert_eq!(Rc::strong_count(&replacement), 1);
        assert_eq!(Rc::strong_count(&second_user), 1);
    }

    #[test]
    fn native_binding_rejects_local_source_without_opening_microphone() {
        use livekit::prelude::LocalAudioTrack;
        use livekit::webrtc::audio_source::native::NativeAudioSource;
        use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
        // Synthetic source, no PlatformAudio, room, capture or physical device.
        for _ in 0..16 {
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let track =
                LocalAudioTrack::create_audio_track("test-local", RtcAudioSource::Native(source));
            assert!(!track.rtc_track().set_playback_volume(0.35));
        }
    }
}
