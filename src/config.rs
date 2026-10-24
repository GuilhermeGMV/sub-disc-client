//! Configuração local: servidor, usuário e cookie de sessão em
//! `$XDG_CONFIG_HOME/subdisc-client/config.json` (permissão 0600) ou, no
//! Windows, `%APPDATA%\subdisc-client\config.json` (pasta privada do usuário).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub server: String,
    pub username: String,
    /// Valor do cookie `subdisc_session`; dá acesso à conta, nunca é registrado em log.
    pub session: Option<String>,
    /// Origem normalizada do servidor → ID estável de usuário → ganhos locais.
    #[serde(default)]
    pub volumes: std::collections::HashMap<String, crate::audio::Volumes>,
}

pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)?;
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("subdisc-client"))
}

fn path() -> Option<PathBuf> {
    Some(dir()?.join("config.json"))
}

impl Config {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(tmp, path)
        })();
        if let Err(e) = result {
            log::warn!("não foi possível salvar a configuração: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioKind, Volume};

    #[test]
    fn legacy_configuration_and_server_isolation() {
        let mut config: Config =
            serde_json::from_str(r#"{"server":"https://a","username":"ana","session":"example"}"#)
                .unwrap();
        assert!(config.volumes.is_empty());
        config
            .volumes
            .entry("https://a".into())
            .or_default()
            .entry("u".into())
            .or_default()
            .set(
                AudioKind::ScreenShare,
                Volume {
                    gain: 0.35,
                    muted: true,
                },
            );
        config
            .volumes
            .entry("https://b".into())
            .or_default()
            .entry("u".into())
            .or_default();
        let restored: Config =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.username, "ana");
        assert_eq!(restored.session.as_deref(), Some("example"));
        assert_eq!(restored.volumes["https://a"]["u"].screen_share.gain, 0.35);
        assert!(restored.volumes["https://a"]["u"].screen_share.muted);
        assert_eq!(restored.volumes["https://b"]["u"].screen_share.gain, 1.0);
    }
}
