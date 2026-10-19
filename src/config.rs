//! Configuração local: servidor, usuário e cookie de sessão em
//! `$XDG_CONFIG_HOME/subdisc-client/config.json`, com permissão 0600.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub server: String,
    pub username: String,
    /// Valor do cookie `subdisc_session`; dá acesso à conta, nunca é registrado em log.
    pub session: Option<String>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("subdisc-client").join("config.json"))
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
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(tmp, path)
        })();
        if let Err(e) = result {
            log::warn!("não foi possível salvar a configuração: {e}");
        }
    }
}
