//! The HTTP client behind every command.
//!
//! # Why the token never reaches the frontend
//!
//! The obvious design has the webview hold the session token and call the
//! server directly. It is one fewer hop, and it is what most of the web does.
//!
//! It is rejected here because a Tauri webview renders imported content: a web
//! page the reader saved, a transcription of a photograph. Any script that
//! finds its way into that context can read anything JavaScript can reach.
//! Keeping the token in Rust means the worst such a script can do is make
//! requests through the commands this application exposes — it cannot take the
//! credential somewhere else.
//!
//! It also means no CORS configuration on the server, because the browser is
//! never the thing making the request.

use std::sync::RwLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{ClientError, Result};

/// Where the library lives, and who we are to it.
pub struct Session {
    base_url: RwLock<String>,
    /// Held here and nowhere else. Never serialised to the frontend, never
    /// returned by a command, never logged.
    token: RwLock<Option<String>>,
    http: reqwest::Client,
}

/// Everything about the signed-in user that the frontend is allowed to know.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
}

#[derive(Deserialize)]
struct SignInResponse {
    token: String,
    #[allow(dead_code)]
    expires_at: String,
    user: Account,
}

/// The server's error shape, so a 400 arrives as its message rather than as
/// "request failed with status 400".
#[derive(Deserialize)]
struct ServerError {
    error: String,
}

impl Session {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: RwLock::new(normalise_url(&base_url.into())),
            token: RwLock::new(None),
            http: reqwest::Client::builder()
                // Generous: a cold model load plus a full page of
                // transcription runs to minutes.
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("failed to build http client"),
        }
    }

    pub fn base_url(&self) -> String {
        self.base_url.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_base_url(&self, url: &str) {
        *self.base_url.write().unwrap_or_else(|e| e.into_inner()) = normalise_url(url);
        // A token is only valid for the server that issued it.
        self.clear_token();
    }

    pub fn is_signed_in(&self) -> bool {
        self.token
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    fn set_token(&self, token: String) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = Some(token);
    }

    pub fn clear_token(&self) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn token(&self) -> Option<String> {
        self.token
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{}", self.base_url(), path)
    }

    /// A request carrying the session token, if there is one.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, self.url(path));
        if let Some(token) = self.token() {
            req = req.bearer_auth(token);
        }
        req
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let response = req.send().await.map_err(|e| ClientError::Unreachable {
            url: self.base_url(),
            source: e,
        })?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status.is_success() {
            // 204 and friends have no body, and the caller for those asks for
            // `()`, which serde deserialises from "null" rather than "".
            let text = if body.trim().is_empty() { "null" } else { &body };
            return serde_json::from_str(text).map_err(|e| ClientError::BadResponse(e.to_string()));
        }

        // A 401 means the token is gone or expired. Dropping it here is what
        // makes the frontend show the sign-in screen rather than failing
        // every subsequent call in the same confusing way.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.clear_token();
        }

        let message = serde_json::from_str::<ServerError>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| {
                if body.trim().is_empty() {
                    format!("the server returned {status}")
                } else {
                    body.clone()
                }
            });

        Err(ClientError::Server {
            status: status.as_u16(),
            message,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.request(reqwest::Method::GET, path)).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.send(self.request(reqwest::Method::POST, path).json(body))
            .await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.send(self.request(reqwest::Method::PUT, path).json(body))
            .await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.request(reqwest::Method::DELETE, path)).await
    }

    /// Raw bytes, for page images.
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let response = self
            .request(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(|e| ClientError::Unreachable {
                url: self.base_url(),
                source: e,
            })?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.clear_token();
        }
        if !response.status().is_success() {
            return Err(ClientError::Server {
                status: response.status().as_u16(),
                message: "could not fetch the image".into(),
            });
        }

        Ok(response
            .bytes()
            .await
            .map_err(|e| ClientError::BadResponse(e.to_string()))?
            .to_vec())
    }

    // --- Sign in ------------------------------------------------------------

    pub async fn sign_in(&self, email: &str, password: &str) -> Result<Account> {
        let label = hostname();
        let body = serde_json::json!({
            "email": email,
            "password": password,
            "label": label,
        });

        // Sent without a token and stored, not returned: the only thing the
        // caller gets back is who they now are.
        let response: SignInResponse = self.send(
            self.http.post(self.url("/sign-in")).json(&body),
        )
        .await?;

        self.set_token(response.token);
        Ok(response.user)
    }

    pub async fn register(
        &self,
        email: &str,
        display_name: &str,
        password: &str,
    ) -> Result<Account> {
        let body = serde_json::json!({
            "email": email,
            "display_name": display_name,
            "password": password,
        });

        let response: SignInResponse =
            self.send(self.http.post(self.url("/register")).json(&body)).await?;

        self.set_token(response.token);
        Ok(response.user)
    }

    pub async fn sign_out(&self) -> Result<()> {
        // Best effort: the local token is dropped whatever the server says,
        // because a reader who pressed sign out is signed out.
        let _: std::result::Result<serde_json::Value, _> =
            self.send(self.request(reqwest::Method::POST, "/sign-out")).await;
        self.clear_token();
        Ok(())
    }
}

/// Accept `192.168.1.50:7878` as readily as a full URL, and drop a trailing
/// slash so paths do not end up doubled.
fn normalise_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return DEFAULT_SERVER.to_string();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    format!("http://{trimmed}")
}

/// Names the session in the server's list, so a reader can tell their laptop
/// from their desktop when reviewing what is signed in.
fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "a reading machine".into())
}

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:7878";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_address_gets_a_scheme() {
        assert_eq!(normalise_url("192.168.4.252:7878"), "http://192.168.4.252:7878");
        assert_eq!(normalise_url("http://x:7878/"), "http://x:7878");
        assert_eq!(normalise_url("https://reading.example.com"), "https://reading.example.com");
        assert_eq!(normalise_url("   "), DEFAULT_SERVER);
    }

    /// Changing servers must not carry the old server's token along: it would
    /// be rejected, and the rejection would look like a broken address.
    #[test]
    fn changing_the_server_forgets_the_token() {
        let s = Session::new("http://a:7878");
        s.set_token("secret".into());
        assert!(s.is_signed_in());

        s.set_base_url("http://b:7878");
        assert!(!s.is_signed_in());
        assert_eq!(s.base_url(), "http://b:7878");
    }

    #[test]
    fn the_token_is_not_reachable_from_outside() {
        let s = Session::new(DEFAULT_SERVER);
        s.set_token("secret".into());
        // `is_signed_in` is the only thing that observes it, and it is a bool.
        assert!(s.is_signed_in());
        s.clear_token();
        assert!(!s.is_signed_in());
    }
}
