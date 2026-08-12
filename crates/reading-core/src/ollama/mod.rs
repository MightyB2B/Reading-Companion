//! Ollama HTTP client.
//!
//! Every option in [`OcrOptions`] is here because a experiment showed it was
//! needed — see the notes on each field. Small OCR models are not well behaved
//! by default, and the difference between a usable app and an unusable one is
//! almost entirely in the guardrails.

use base64::Engine;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";
pub const DEFAULT_OCR_MODEL: &str = "glm-ocr";
pub const DEFAULT_TEXT_MODEL: &str = "qwen3:4b";

/// Model used only to read the page number off the image.
///
/// `glm-ocr` cannot do this job: asked "what page number is printed here" it
/// ignores the question and transcribes the whole page, because it is an OCR
/// model rather than an instruction-follower. `qwen3-vl:4b` answered correctly
/// on both halves of a photographed spread, returning the bare number in about
/// a second.
pub const DEFAULT_VISION_MODEL: &str = "qwen3-vl:4b";

/// How long an idle model stays loaded, sent with every request.
///
/// Long enough that switching between transcribing a page and critiquing a
/// summary never pays a model reload; short enough that a laptop gets its VRAM
/// back after a reading session. With glm-ocr at 2.2GB and qwen3:4b at 2.5GB
/// both stay resident inside 8GB with room for context.
///
/// A dedicated server has nothing to give the memory back to, so this is a
/// setting: `-1` pins models indefinitely. Ollama's own `OLLAMA_KEEP_ALIVE`
/// cannot do that job, because a value sent on the request wins over it.
pub const DEFAULT_KEEP_ALIVE: &str = "30m";

/// A model installed on the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
    /// e.g. "4.0B". Empty when the server does not report it.
    pub parameter_size: String,
    /// e.g. "Q4_K_M".
    pub quantization: String,
    /// e.g. ["vision", "completion", "tools", "thinking"].
    pub capabilities: Vec<String>,
}

impl ModelInfo {
    /// Can this model be shown an image?
    pub fn sees_images(&self) -> bool {
        self.capabilities.iter().any(|c| c == "vision")
    }
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    host: String,
    keep_alive: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
    stream: bool,
    keep_alive: &'a str,
    /// qwen3 is a reasoning model. Left enabled its chain of thought is
    /// emitted alongside the answer and corrupts the JSON, so structured calls
    /// turn it off explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<serde_json::Value>,
    options: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Clone)]
pub struct OcrOptions {
    /// Bounds a runaway. A book page is roughly 1000-1500 tokens; glm-ocr was
    /// observed generating 59,000 characters over 124 seconds on one fixture
    /// when left uncapped. This turns that failure into a two-second one.
    pub num_predict: i32,
    pub num_ctx: i32,
    pub temperature: f32,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            num_predict: 2048,
            num_ctx: 8192,
            temperature: 0.0,
        }
    }
}

/// The transcription prompt.
///
/// The "once, then stop" instruction is load-bearing: without it glm-ocr
/// reliably transcribed the page and then began again from the top. With it,
/// repetition disappeared on the modern fixture entirely and was reduced to a
/// single duplicate on the early-modern one, which segmentation then removes.
///
/// Asking for a diplomatic transcription is deliberately *not* attempted.
/// glm-ocr silently modernises long-s and ligatures no matter how it is asked,
/// so the original photograph — not the text — is the record of what was
/// actually printed.
pub const OCR_PROMPT: &str = "\
Transcribe all text from this book page.

Rules:
- Preserve paragraph structure. Separate paragraphs with a blank line.
- If a page number is printed in the margin, put it alone on the first line.
- Do not repeat any paragraph.
- Output the transcription once, then stop.
- Output only the transcription. Never describe the image, never explain what \
you could or could not read, and never apologise. If the text runs off the \
page, simply stop.
- No commentary, no code fences.";

impl OllamaClient {
    /// `api_key` is for hosted Ollama-compatible servers, which authenticate
    /// with a bearer token. A local install needs none, so it is optional.
    ///
    /// The key is attached as a default header rather than per request: every
    /// endpoint this client touches needs it, and a call site that forgot
    /// would fail with a 401 that reads like a wrong key rather than a
    /// missing one.
    pub fn new(host: impl Into<String>, api_key: Option<&str>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
            // A key with a stray newline or non-ASCII character cannot go in a
            // header at all. Dropping it gives an honest 401 instead of every
            // request failing to build for reasons the reader cannot see.
            if let Ok(mut value) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            {
                value.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }

        Self {
            host: host.into(),
            keep_alive: DEFAULT_KEEP_ALIVE.to_string(),
            http: reqwest::Client::builder()
                .default_headers(headers)
                // Generous, because a cold model load can take ~50s. The real
                // protection against a runaway is num_predict, not this.
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build http client"),
        }
    }

    /// How long the server should hold a model after this client's requests.
    /// Blank falls back to the default rather than sending an empty string,
    /// which Ollama reads as zero and unloads immediately.
    pub fn with_keep_alive(mut self, keep_alive: &str) -> Self {
        let trimmed = keep_alive.trim();
        if !trimmed.is_empty() {
            self.keep_alive = trimmed.to_string();
        }
        self
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn keep_alive(&self) -> &str {
        &self.keep_alive
    }

    /// Is Ollama up? Used at startup to give a clear message rather than
    /// letting the first real call fail confusingly.
    pub async fn health(&self) -> Result<Vec<String>> {
        Ok(self
            .installed_models()
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect())
    }

    /// Every model installed on the server, with enough detail to choose
    /// between them.
    ///
    /// `capabilities` is the field that matters: it says outright whether a
    /// model can see an image. Transcribing a page with a text-only model
    /// fails in a way that looks like a bug in this application rather than a
    /// wrong choice, so the settings window filters by it.
    pub async fn installed_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.host);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnreachable {
                url: self.host.clone(),
                source,
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::OllamaStatus { status, body });
        }

        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Model>,
        }
        #[derive(Deserialize)]
        struct Model {
            name: String,
            #[serde(default)]
            size: u64,
            #[serde(default)]
            details: Details,
            #[serde(default)]
            capabilities: Vec<String>,
        }
        #[derive(Deserialize, Default)]
        struct Details {
            #[serde(default)]
            parameter_size: String,
            #[serde(default)]
            quantization_level: String,
        }

        let tags: Tags = resp.json().await?;
        let mut models: Vec<ModelInfo> = tags
            .models
            .into_iter()
            .map(|m| ModelInfo {
                name: m.name,
                size_bytes: m.size,
                parameter_size: m.details.parameter_size,
                quantization: m.details.quantization_level,
                capabilities: m.capabilities,
            })
            .collect();

        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    /// Streaming generate. `on_chunk` receives text as it arrives so the UI can
    /// show progress instead of a spinner; the full text is returned at the end.
    pub async fn generate_stream<F>(
        &self,
        model: &str,
        prompt: &str,
        system: Option<&str>,
        images: Vec<Vec<u8>>,
        options: serde_json::Value,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let encoded: Vec<String> = images
            .iter()
            .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
            .collect();

        let req = GenerateRequest {
            model,
            prompt,
            system,
            images: encoded,
            stream: true,
            keep_alive: &self.keep_alive,
            think: None,
            format: None,
            options,
        };

        let url = format!("{}/api/generate", self.host);
        let resp = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnreachable {
                url: self.host.clone(),
                source,
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::OllamaStatus { status, body });
        }

        // NDJSON: one JSON object per line, but chunks can split mid-line.
        let mut full = String::new();
        let mut buf = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(item) = stream.next().await {
            let bytes = item.map_err(|source| AppError::OllamaUnreachable {
                url: self.host.clone(),
                source,
            })?;
            buf.extend_from_slice(&bytes);

            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let chunk: GenerateChunk = match serde_json::from_str(line) {
                    Ok(c) => c,
                    // A malformed line mid-stream is not worth aborting over.
                    Err(_) => continue,
                };
                if !chunk.response.is_empty() {
                    on_chunk(&chunk.response);
                    full.push_str(&chunk.response);
                }
                if chunk.done {
                    return Ok(full);
                }
            }
        }
        Ok(full)
    }

    /// Transcribe one page image.
    pub async fn ocr<F>(
        &self,
        model: &str,
        image: Vec<u8>,
        opts: &OcrOptions,
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let options = serde_json::json!({
            "temperature": opts.temperature,
            "num_ctx": opts.num_ctx,
            "num_predict": opts.num_predict,
        });
        self.generate_stream(model, OCR_PROMPT, None, vec![image], options, on_chunk)
            .await
    }

    /// Ask a vision model a short question about an image.
    ///
    /// Deliberately capped at a handful of tokens: the only question asked
    /// this way is "what page number is printed here", and a bounded answer
    /// keeps a chatty model from narrating the page.
    pub async fn ask_about_image(
        &self,
        model: &str,
        prompt: &str,
        image: Vec<u8>,
    ) -> Result<String> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&image);
        let req = GenerateRequest {
            model,
            prompt,
            system: None,
            images: vec![encoded],
            stream: false,
            keep_alive: &self.keep_alive,
            think: Some(false),
            format: None,
            options: serde_json::json!({
                "temperature": 0,
                "num_ctx": 4096,
                "num_predict": 24,
            }),
        };

        let url = format!("{}/api/generate", self.host);
        let resp = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnreachable {
                url: self.host.clone(),
                source,
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::OllamaStatus { status, body });
        }

        #[derive(Deserialize)]
        struct Full {
            response: String,
        }
        Ok(resp.json::<Full>().await?.response.trim().to_string())
    }

    /// Generate a value conforming to a JSON schema, retrying once if the
    /// model produces something unparseable.
    ///
    /// The retry is not superstition. A small model that falls into a
    /// repetition loop at a fixed low temperature will reproduce the same loop
    /// on an identical request, so the retry nudges the temperature up to
    /// break it. Observed with qwen3:4b looping inside a string field until it
    /// exhausted num_predict and left the JSON unterminated.
    pub async fn generate_structured<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: serde_json::Value,
    ) -> Result<T> {
        match self
            .generate_structured_once(model, system, prompt, schema.clone(), 0.2)
            .await
        {
            Err(AppError::BadModelOutput(_)) => {
                self.generate_structured_once(model, system, prompt, schema, 0.6)
                    .await
            }
            other => other,
        }
    }

    async fn generate_structured_once<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: serde_json::Value,
        temperature: f32,
    ) -> Result<T> {
        let req = GenerateRequest {
            model,
            prompt,
            system: Some(system),
            images: Vec::new(),
            stream: false,
            keep_alive: &self.keep_alive,
            think: Some(false),
            format: Some(schema),
            options: serde_json::json!({
                "temperature": temperature,
                "num_ctx": 8192,
                "num_predict": 1024,
            }),
        };

        let url = format!("{}/api/generate", self.host);
        let resp = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnreachable {
                url: self.host.clone(),
                source,
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::OllamaStatus { status, body });
        }

        #[derive(Deserialize)]
        struct Full {
            response: String,
        }
        let full: Full = resp.json().await?;

        // Some models wrap JSON in a fence despite the format constraint.
        let cleaned = strip_code_fence(full.response.trim());
        serde_json::from_str(cleaned).map_err(|e| {
            AppError::BadModelOutput(format!("{e}; model returned: {}", truncate(cleaned, 400)))
        })
    }
}

fn strip_code_fence(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    // Drop an optional language tag on the opening fence.
    let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or(rest);
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "..."
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::new(DEFAULT_HOST, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_json_code_fence() {
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn ocr_defaults_bound_a_runaway() {
        // Regression guard: an uncapped num_predict produced a 59k-character,
        // 124-second transcription of a single page during evaluation.
        let o = OcrOptions::default();
        assert!(o.num_predict > 0 && o.num_predict <= 4096);
        assert_eq!(o.temperature, 0.0);
    }

    #[test]
    fn ocr_prompt_tells_the_model_to_stop() {
        // Removing this instruction reintroduced whole-page repetition.
        assert!(OCR_PROMPT.contains("once, then stop"));
        assert!(OCR_PROMPT.contains("Do not repeat"));
    }

    #[test]
    fn ocr_prompt_forbids_commentary_and_asks_for_the_folio() {
        // glm-ocr was observed apologising in prose when it ran out of tokens,
        // and omitting the page number from the running head.
        assert!(OCR_PROMPT.contains("never apologise"));
        assert!(OCR_PROMPT.contains("page number"));
    }
}
