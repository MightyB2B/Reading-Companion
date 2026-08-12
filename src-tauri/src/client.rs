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
    /// The server's certificate, when it is one no public authority signed.
    /// Kept so the client can be rebuilt if the address changes.
    root_cert: RwLock<Option<String>>,
    http: RwLock<reqwest::Client>,
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

/// Build the HTTP client, trusting `root_cert` alongside the system roots.
///
/// Added as an extra root rather than switching verification off. A library
/// server on a home network will usually have a certificate no public
/// authority signed, and the tempting fix — accept anything — would mean
/// anything on that network could impersonate the server and read everything
/// you read. Trusting one specific certificate keeps the guarantee.
fn build_http(root_cert: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        // Generous: a cold model load plus a full page of transcription runs
        // to minutes.
        .timeout(std::time::Duration::from_secs(600));

    if let Some(pem) = root_cert {
        match reqwest::Certificate::from_pem(pem.as_bytes()) {
            Ok(cert) => builder = builder.add_root_certificate(cert),
            // Dropped rather than fatal: the connection then fails with a TLS
            // error the reader can act on, instead of the application
            // refusing to start.
            Err(e) => eprintln!("ignoring an unreadable server certificate: {e}"),
        }
    }

    builder.build().expect("failed to build http client")
}

impl Session {
    pub fn new(base_url: impl Into<String>, root_cert: Option<String>) -> Self {
        Self {
            base_url: RwLock::new(normalise_url(&base_url.into())),
            token: RwLock::new(None),
            http: RwLock::new(build_http(root_cert.as_deref())),
            root_cert: RwLock::new(root_cert),
        }
    }

    /// Trust this certificate for the library server.
    ///
    /// Returns the PEM actually stored, so the caller persists exactly what
    /// took effect. `None` or empty goes back to the system roots alone.
    pub fn set_root_cert(&self, pem: Option<String>) -> Result<Option<String>> {
        let cleaned = pem.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());

        if let Some(p) = &cleaned {
            check_certificate(p)?;
        }

        *self.http.write().unwrap_or_else(|e| e.into_inner()) = build_http(cleaned.as_deref());
        *self.root_cert.write().unwrap_or_else(|e| e.into_inner()) = cleaned.clone();
        Ok(cleaned)
    }

    pub fn has_root_cert(&self) -> bool {
        self.root_cert
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Cloning is cheap: `reqwest::Client` is an Arc inside.
    fn http(&self) -> reqwest::Client {
        self.http.read().unwrap_or_else(|e| e.into_inner()).clone()
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
        let mut req = self.http().request(method, self.url(path));
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
            self.http().post(self.url("/sign-in")).json(&body),
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
            self.send(self.http().post(self.url("/register")).json(&body)).await?;

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

/// Is this a certificate the reader meant to pick?
///
/// Checked when it is chosen rather than at the next request, so a wrong file
/// is reported while the file picker is still on screen instead of arriving
/// later as a connection failure.
///
/// `reqwest::Certificate::from_pem` alone is not enough: it accepts input with
/// no PEM structure at all and defers the real parsing, so the two realistic
/// mistakes — picking some other file, or picking the *key* instead of the
/// certificate — would both be stored happily and fail much later.
fn check_certificate(pem: &str) -> Result<()> {
    use base64::Engine;

    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let Some(start) = pem.find(BEGIN) else {
        // Naming the likely mistake is worth more than naming the rule.
        let hint = if pem.contains("PRIVATE KEY") {
            " That looks like the private key. The certificate is the other \
             file, and it is the one meant to leave the server."
        } else {
            ""
        };
        return Err(ClientError::Invalid(format!(
            "that file does not contain a certificate.{hint}"
        )));
    };

    let body_start = start + BEGIN.len();
    let end = pem[body_start..]
        .find(END)
        .map(|i| body_start + i)
        .ok_or_else(|| {
            ClientError::Invalid("that certificate is cut off before it ends.".into())
        })?;

    // The base64 has to decode to something shaped like a certificate.
    // `reqwest::Certificate::from_pem` will not tell us: it stores the bytes
    // and defers every check to when the client is built, so a truncated or
    // corrupted file passes it and fails much later as a connection error.
    let body: String = pem[body_start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    let der = base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|_| {
            ClientError::Invalid("that certificate is damaged and cannot be read.".into())
        })?;

    // A DER certificate is a SEQUENCE, so it starts 0x30, and no real one is
    // anywhere near this short.
    if der.len() < 100 || der[0] != 0x30 {
        return Err(ClientError::Invalid(
            "that does not look like a certificate.".into(),
        ));
    }

    reqwest::Certificate::from_pem(pem.as_bytes())
        .map(|_| ())
        .map_err(|e| ClientError::Invalid(format!("that certificate could not be read: {e}")))
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
        let s = Session::new("http://a:7878", None);
        s.set_token("secret".into());
        assert!(s.is_signed_in());

        s.set_base_url("http://b:7878");
        assert!(!s.is_signed_in());
        assert_eq!(s.base_url(), "http://b:7878");
    }

    /// A wrong file has to be refused at the moment it is chosen. Storing it
    /// and failing at the next request would report a connection problem for
    /// what is really a file-picker mistake.
    #[test]
    fn an_unreadable_certificate_is_refused() {
        let s = Session::new(DEFAULT_SERVER, None);

        assert!(s.set_root_cert(Some("not a certificate".into())).is_err());
        assert!(s.set_root_cert(Some("".into())).is_ok(), "empty means none");

        // And nothing was kept from the attempts.
        assert!(!s.has_root_cert());
    }

    /// Both files sit side by side with similar names, so picking the key is
    /// the mistake most likely to actually happen — and it is the one that
    /// must not be shrugged off, since the key is the secret.
    #[test]
    fn picking_the_private_key_says_so() {
        let key = "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----";
        let err = check_certificate(key).unwrap_err().to_string();
        assert!(err.contains("private key"), "unhelpful message: {err}");
    }

    /// A real certificate, made the way install-server.ps1 makes them.
    ///
    /// Expired long before anyone reads this and useful for nothing but this
    /// test: the point is that the validation above accepts the genuine
    /// article, which every rejection test on its own could not tell us.
    const REAL_CERT: &str = "\
-----BEGIN CERTIFICATE-----
MIIDNzCCAh+gAwIBAgIQFLc+NyaSxadJgRSPTNiu6TANBgkqhkiG9w0BAQsFADAcMRowGAYDVQQD
DBFSZWFkaW5nIENvbXBhbmlvbjAeFw0yNjA4MTIyMDI5MjBaFw0zNjA4MTIyMDM5MTlaMBwxGjAY
BgNVBAMMEVJlYWRpbmcgQ29tcGFuaW9uMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA
unkBW5NHqDKPG0yWXiF24Yodzq6A+rhM0Dp/HlWAtjIcmua8TIoyhicVJp+PYB0KMqH0wKV/6sjT
E9K7njGq8SdT8DtCgDv1fz9sAeJRfoZgMk29uElhDWPfu/EM2ELz1zpL8XL8BdLUekpbuCPExwld
zaM5Ea/QiLZBu0TS2BxrwwqJ0SvVEbVNFIm7zYIiereuUwqDFUyQp1GO+fmPiE4AcmBcDiPFE9Wv
y1ikUofLwlPrQuNXFjQojzWVSpoz+AfthH5xjYT/+tj4dX1N0mkn9X5YJ5m3+v3ySMZpgvjQqt4j
hP89XwlVVHM+YYeo2owcXfuTyrYW/IVfqJBv6QIDAQABo3UwczAOBgNVHQ8BAf8EBAMCBaAwHQYD
VR0lBBYwFAYIKwYBBQUHAwIGCCsGAQUFBwMBMCMGA1UdEQQcMBqCCWxvY2FsaG9zdIINMTkyLjE2
OC40LjI1MjAdBgNVHQ4EFgQUdKSgWpl2I2I2I4S4uOBy8JH5jHwwDQYJKoZIhvcNAQELBQADggEB
ADPP0GO1na28mUnVMqOals4Pp001cH2KHMkvURI9JwLFbuwebSl8AzBvdPSEkF0J4WmBy2YFSb9K
GGBD4q8d0nDA4htTun9h1G16f72Kf25RaYHLHgVPC7zigHV87u0X+kZOGZAVZP6kZoa/hOeM1Hd8
oO/4dbfhNuRSC/BOs12PB/+xxKd3CHi9mj65NghUqh/5rJtNbXvfaS41Fmzm+mBnXS/FQ/9d/Qob
jWzME77T1+jtEz9AakoVeeb+PSYcvBF5wMqKpF7NBgzZAyE7Ov76AtbU8Rq1uSbOHO7s1LVRv5DP
1p+UkaLlkRpLBIePDQjo1O/vjgbh3NKgr68UIC0=
-----END CERTIFICATE-----
";

    #[test]
    fn a_real_certificate_is_accepted() {
        check_certificate(REAL_CERT).expect("a genuine certificate was refused");

        let s = Session::new(DEFAULT_SERVER, None);
        s.set_root_cert(Some(REAL_CERT.into())).unwrap();
        assert!(s.has_root_cert());

        // And it can be given up again.
        s.set_root_cert(None).unwrap();
        assert!(!s.has_root_cert());
    }

    /// Trailing whitespace, CRLF endings, and a stray blank line are what a
    /// file that has been through Notepad and a network share looks like.
    #[test]
    fn a_certificate_survives_being_copied_about() {
        let mangled = REAL_CERT.replace('\n', "\r\n");
        check_certificate(&mangled).expect("CRLF endings were refused");

        let padded = format!("\n\n{REAL_CERT}   \n");
        check_certificate(&padded).expect("surrounding whitespace was refused");
    }

    #[test]
    fn a_file_with_no_certificate_in_it_is_refused() {
        assert!(check_certificate("hello").is_err());
        assert!(check_certificate("").is_err());
        // Structure alone is not enough; the contents still have to parse.
        assert!(
            check_certificate("-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----")
                .is_err()
        );
    }

    #[test]
    fn no_certificate_and_an_empty_one_mean_the_same_thing() {
        let s = Session::new(DEFAULT_SERVER, None);
        assert!(!s.has_root_cert());

        assert_eq!(s.set_root_cert(None).unwrap(), None);
        assert_eq!(s.set_root_cert(Some("   ".into())).unwrap(), None);
        assert!(!s.has_root_cert());
    }

    #[test]
    fn the_token_is_not_reachable_from_outside() {
        let s = Session::new(DEFAULT_SERVER, None);
        s.set_token("secret".into());
        // `is_signed_in` is the only thing that observes it, and it is a bool.
        assert!(s.is_signed_in());
        s.clear_token();
        assert!(!s.is_signed_in());
    }
}
