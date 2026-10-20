//! Contrato da seleção. Enumerar nunca inicia captura nem solicita mídia.
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

#[derive(Clone, Debug)]
pub struct ShareRequest {
    pub source: CaptureSourceInfo,
    pub video: bool,
    pub audio: bool,
}

impl ShareRequest {
    pub fn validate(&self, audio_available: bool) -> Result<(), String> {
        if !self.video && !self.audio {
            return Err("Habilite vídeo ou áudio para compartilhar.".into());
        }
        if self.audio && !audio_available {
            return Err("Captura de áudio da tela indisponível neste cliente. O microfone continua independente.".into());
        }
        Ok(())
    }
}

pub fn uses_portal() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn enumerate() -> Result<Vec<CaptureSourceInfo>, String> {
    if uses_portal() {
        return Ok(vec![CaptureSourceInfo {
            id: 0,
            title: "Escolher no portal do sistema…".into(),
            kind: CaptureKind::Portal,
        }]);
    }
    #[cfg(target_os = "linux")]
    {
        use x11rb::{
            connection::Connection,
            protocol::{randr::ConnectionExt as _, xproto::ConnectionExt as _},
        };
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let mut sources = vec![];
        let monitors = conn
            .randr_get_monitors(root, true)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        for (index, monitor) in monitors.monitors.iter().enumerate() {
            sources.push(CaptureSourceInfo {
                id: monitor.name as u64,
                title: format!(
                    "Monitor {} ({}×{})",
                    index + 1,
                    monitor.width,
                    monitor.height
                ),
                kind: CaptureKind::Monitor,
            });
        }
        let list = conn
            .intern_atom(false, b"_NET_CLIENT_LIST")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        let name = conn
            .intern_atom(false, b"_NET_WM_NAME")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        let list_reply = conn
            .get_property(
                false,
                root,
                list,
                x11rb::protocol::xproto::AtomEnum::WINDOW,
                0,
                4096,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let mut windows = list_reply
            .value32()
            .map(|v| v.collect::<Vec<_>>())
            .unwrap_or_default();
        if windows.is_empty() {
            windows = conn
                .query_tree(root)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
                .children;
        }
        for window in windows {
            let visible = conn
                .get_window_attributes(window)
                .map_err(|e| e.to_string())?
                .reply()
                .is_ok_and(|v| v.map_state == x11rb::protocol::xproto::MapState::VIEWABLE);
            if !visible {
                continue;
            }
            for property in [name, x11rb::protocol::xproto::AtomEnum::WM_NAME.into()] {
                if let Ok(reply) = conn
                    .get_property(
                        false,
                        window,
                        property,
                        x11rb::protocol::xproto::AtomEnum::ANY,
                        0,
                        1024,
                    )
                    .map_err(|e| e.to_string())?
                    .reply()
                {
                    let title = String::from_utf8_lossy(&reply.value).to_string();
                    if !title.trim().is_empty() {
                        sources.push(CaptureSourceInfo {
                            id: window as u64,
                            title,
                            kind: CaptureKind::Window,
                        });
                        break;
                    }
                }
            }
        }
        return if sources.is_empty() {
            Err("Nenhuma fonte disponível".into())
        } else {
            Ok(sources)
        };
    }
    #[cfg(target_os = "windows")]
    {
        return crate::screen::windows_sources();
    }
    #[allow(unreachable_code)]
    Err("Captura não suportada neste sistema".into())
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
        for (video, audio) in [(true, true), (true, false), (false, true), (false, false)] {
            let request = ShareRequest {
                source: CaptureSourceInfo {
                    id: 1,
                    title: "Tela".into(),
                    kind: CaptureKind::Monitor,
                },
                video,
                audio,
            };
            assert_eq!(request.validate(true).is_ok(), video || audio);
            assert_eq!(request.validate(false).is_ok(), video && !audio);
        }
    }
}
