//! Cliente HTTP da API `/api`. O servidor exige `Origin` igual a `APP_ORIGIN` nas
//! mutações e autentica pelo cookie `subdisc_session`, que é mantido aqui à mão
//! para poder ser reutilizado no WebSocket e salvo na configuração.
use crate::model::{ChatMessage, JoinResponse, Snapshot, User};
use reqwest::{Method, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct ApiError {
    pub status: Option<StatusCode>,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    origin: String,
    session: Arc<RwLock<Option<String>>>,
}

/// Normaliza o endereço digitado para a origem exata (`https://host[:porta]`).
pub fn normalize_origin(input: &str) -> Result<String, String> {
    let input = input.trim().trim_end_matches('/');
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|_| "Endereço inválido".to_owned())?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return Err("Use um endereço http(s)://".into());
    }
    Ok(url.origin().ascii_serialization())
}

impl Api {
    pub fn new(origin: String, session: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("subdisc-client/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("cliente HTTP");
        Self {
            http,
            origin,
            session: Arc::new(RwLock::new(session)),
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn session(&self) -> Option<String> {
        self.session.read().unwrap().clone()
    }

    pub fn cookie_header(&self) -> Option<String> {
        self.session().map(|s| format!("subdisc_session={s}"))
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> ApiResult<T> {
        let mut req = self
            .http
            .request(method, format!("{}/api{path}", self.origin))
            .header(header::ORIGIN, &self.origin);
        if let Some(cookie) = self.cookie_header() {
            req = req.header(header::COOKIE, cookie);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.map_err(|e| ApiError {
            status: None,
            message: if e.is_connect() || e.is_timeout() {
                "Servidor inacessível".into()
            } else {
                format!("Falha de rede: {e}")
            },
        })?;
        let status = res.status();
        for value in res.headers().get_all(header::SET_COOKIE) {
            let Ok(value) = value.to_str() else { continue };
            if let Some(rest) = value.strip_prefix("subdisc_session=") {
                let raw = rest.split(';').next().unwrap_or_default();
                *self.session.write().unwrap() = (!raw.is_empty()).then(|| raw.to_owned());
            }
        }
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                .unwrap_or_else(|| match status {
                    StatusCode::UNAUTHORIZED => "Sessão expirada".into(),
                    StatusCode::TOO_MANY_REQUESTS => "Muitas tentativas; aguarde".into(),
                    _ => format!("Erro do servidor ({status})"),
                });
            return Err(ApiError {
                status: Some(status),
                message,
            });
        }
        serde_json::from_str(&text).map_err(|_| ApiError {
            status: Some(status),
            message: "Resposta inválida do servidor".into(),
        })
    }

    pub async fn needs_setup(&self) -> ApiResult<bool> {
        let v: Value = self.request(Method::GET, "/setup", None).await?;
        Ok(v["needs_setup"].as_bool().unwrap_or(false))
    }

    pub async fn login(&self, username: &str, password: &str) -> ApiResult<User> {
        let body = json!({ "username": username, "password": password });
        self.request(Method::POST, "/login", Some(body)).await
    }

    pub async fn setup(&self, username: &str, password: &str, setup_key: &str) -> ApiResult<User> {
        let body = json!({ "username": username, "password": password, "setup_key": setup_key });
        self.request(Method::POST, "/setup", Some(body)).await
    }

    pub async fn register(&self, username: &str, password: &str, invite: &str) -> ApiResult<User> {
        let body = json!({ "username": username, "password": password, "invite": invite });
        self.request(Method::POST, "/register", Some(body)).await
    }

    pub async fn logout(&self) -> ApiResult<()> {
        let _: Value = self
            .request(Method::POST, "/logout", Some(json!({})))
            .await?;
        *self.session.write().unwrap() = None;
        Ok(())
    }

    pub async fn me(&self) -> ApiResult<User> {
        self.request(Method::GET, "/me", None).await
    }

    pub async fn state(&self) -> ApiResult<Snapshot> {
        self.request(Method::GET, "/state", None).await
    }

    pub async fn messages(
        &self,
        channel: &str,
        before: Option<i64>,
    ) -> ApiResult<Vec<ChatMessage>> {
        let query = before.map(|b| format!("?before={b}")).unwrap_or_default();
        self.request(
            Method::GET,
            &format!("/channels/{channel}/messages{query}"),
            None,
        )
        .await
    }

    pub async fn send_message(&self, channel: &str, content: &str) -> ApiResult<ChatMessage> {
        let body = json!({ "content": content });
        self.request(
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(body),
        )
        .await
    }

    pub async fn join(&self, channel: &str) -> ApiResult<JoinResponse> {
        self.request(
            Method::POST,
            &format!("/channels/{channel}/join"),
            Some(json!({})),
        )
        .await
    }

    pub async fn leave(&self, lease: &str) -> ApiResult<()> {
        let _: Value = self
            .request(
                Method::POST,
                "/media/leave",
                Some(json!({ "lease": lease })),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalizes_origin() {
        let n = |s| super::normalize_origin(s);
        assert_eq!(n("chat.exemplo.com").unwrap(), "https://chat.exemplo.com");
        assert_eq!(
            n("https://chat.exemplo.com/").unwrap(),
            "https://chat.exemplo.com"
        );
        assert_eq!(
            n(" http://localhost:8080/app ").unwrap(),
            "http://localhost:8080"
        );
        assert!(n("ftp://x").is_err());
    }
}
