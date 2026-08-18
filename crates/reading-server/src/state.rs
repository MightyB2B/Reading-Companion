//! What every handler shares.

use std::path::PathBuf;
use std::sync::Arc;

use reading_core::db::pg::Db;
use reading_core::dict::Dictionary;
use reading_core::ollama::OllamaClient;
use reading_core::Result;

/// Server-wide configuration, held in the database so it survives a restart
/// and so an administrator can change it without a deploy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Where Ollama is. Not necessarily this machine.
    pub ollama_host: String,
    /// Bearer token for a hosted Ollama. Empty for a local one.
    pub ollama_api_key: String,
    pub ocr_model: String,
    pub text_model: String,
    /// Reads the page number off the image when the transcription has none.
    pub vision_model: String,
    /// How long the inference server holds a model after a request.
    pub keep_alive: String,
    /// May anyone who can reach this server create an account?
    ///
    /// False by default, and deliberately so. The bootstrap case — an empty
    /// server, where the first arrival has to be able to register — is handled
    /// separately, so leaving this off does not lock anyone out of a new
    /// library. What it prevents is a server exposed to a network staying open
    /// to signups forever because nobody thought to close it.
    pub open_registration: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ollama_host: reading_core::ollama::DEFAULT_HOST.to_string(),
            ollama_api_key: String::new(),
            ocr_model: reading_core::ollama::DEFAULT_OCR_MODEL.to_string(),
            text_model: reading_core::ollama::DEFAULT_TEXT_MODEL.to_string(),
            vision_model: reading_core::ollama::DEFAULT_VISION_MODEL.to_string(),
            keep_alive: reading_core::ollama::DEFAULT_KEEP_ALIVE.to_string(),
            open_registration: false,
        }
    }
}

impl Settings {
    /// Read from the database, falling back to the defaults for anything
    /// unset — which is every key on a fresh install.
    pub async fn load(db: &Db) -> Result<Self> {
        let mut settings = Settings::default();

        if let Some(v) = db.get_setting("ollama_host").await? {
            settings.ollama_host = v;
        }
        if let Some(v) = db.get_setting("ollama_api_key").await? {
            settings.ollama_api_key = v;
        }
        if let Some(v) = db.get_setting("ocr_model").await? {
            settings.ocr_model = v;
        }
        if let Some(v) = db.get_setting("text_model").await? {
            settings.text_model = v;
        }
        if let Some(v) = db.get_setting("vision_model").await? {
            settings.vision_model = v;
        }
        if let Some(v) = db.get_setting("keep_alive").await? {
            settings.keep_alive = v;
        }
        if let Some(v) = db.get_setting("open_registration").await? {
            // Anything that is not exactly "true" is false. A malformed value
            // must fail closed: the failure mode of guessing the other way is
            // an open server.
            settings.open_registration = v == "true";
        }

        Ok(settings)
    }

    pub async fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("ollama_host", &self.ollama_host).await?;
        db.set_setting("ollama_api_key", &self.ollama_api_key).await?;
        db.set_setting("ocr_model", &self.ocr_model).await?;
        db.set_setting("text_model", &self.text_model).await?;
        db.set_setting("vision_model", &self.vision_model).await?;
        db.set_setting("keep_alive", &self.keep_alive).await?;
        db.set_setting(
            "open_registration",
            if self.open_registration { "true" } else { "false" },
        )
        .await?;
        Ok(())
    }

    /// Tidy an address typed by a person.
    pub fn normalise_host(input: &str) -> String {
        let trimmed = input.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return reading_core::ollama::DEFAULT_HOST.to_string();
        }
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return trimmed.to_string();
        }
        format!("http://{trimmed}")
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    /// Behind a lock because the address is configurable: pointing the server
    /// at another machine replaces the client rather than restarting.
    pub ollama: Arc<std::sync::RwLock<OllamaClient>>,
    pub settings: Arc<std::sync::Mutex<Settings>>,
    /// Where page images live, one directory per book.
    pub library_dir: PathBuf,
    /// Absent when the dictionary has not been built. Lookups are then
    /// unavailable rather than the server failing to start.
    pub dict: Option<Arc<Dictionary>>,
    /// Slows down guessing at the two endpoints an anonymous request reaches.
    pub throttle: Arc<crate::throttle::Throttle>,
}

impl AppState {
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// A snapshot of the client. Cloning is cheap — `reqwest::Client` is an
    /// Arc inside — and it avoids holding the lock across an await.
    pub fn ollama(&self) -> OllamaClient {
        self.ollama
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn rebuild_ollama(&self, settings: &Settings) {
        *self.ollama.write().unwrap_or_else(|e| e.into_inner()) = OllamaClient::new(
            settings.ollama_host.clone(),
            Some(&settings.ollama_api_key),
        )
        .with_keep_alive(&settings.keep_alive);
    }
}
