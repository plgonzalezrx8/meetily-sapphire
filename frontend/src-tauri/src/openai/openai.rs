use crate::provider_catalog::{recommended_models, ModelCache};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tauri::command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenAIModel {
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIApiModel {
    id: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIApiResponse {
    data: Vec<OpenAIApiModel>,
}

const MODELS_URL: &str = "https://api.openai.com/v1/models";
static MODELS_CACHE: ModelCache<OpenAIModel> = ModelCache::new();

// shortcut: IDs can misclassify capabilities, use API metadata if exposed.
fn is_chat_model(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    let id = id
        .strip_prefix("ft:")
        .and_then(|tuned| tuned.split(':').next())
        .unwrap_or(&id);
    let reasoning_model = id.strip_prefix('o').is_some_and(|suffix| {
        let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && (digits == suffix.len() || suffix.as_bytes()[digits] == b'-')
    });
    let responses_only = [
        "o1-pro",
        "o3-pro",
        "gpt-5-pro",
        "gpt-5.2-pro",
        "gpt-5.4-pro",
        "gpt-5.5-pro",
        "o3-deep-research",
        "o4-mini-deep-research",
    ]
    .iter()
    .any(|family| {
        id == *family
            || id
                .strip_prefix(*family)
                .is_some_and(|suffix| suffix.starts_with('-'))
    });
    (id.starts_with("gpt-") || id.starts_with("chatgpt-") || reasoning_model)
        && !responses_only
        && ![
            "embedding",
            "tts",
            "whisper",
            "dall-e",
            "babbage",
            "davinci",
            "instruct",
            "realtime",
            "audio",
            "transcribe",
            "image",
            "codex",
        ]
        .iter()
        .any(|name| id.contains(name))
}

#[command]
pub async fn get_openai_models(api_key: Option<String>) -> Result<Vec<OpenAIModel>, String> {
    fetch_models(api_key, MODELS_URL, &MODELS_CACHE).await
}

async fn fetch_models(
    api_key: Option<String>,
    endpoint: &str,
    cache: &ModelCache<OpenAIModel>,
) -> Result<Vec<OpenAIModel>, String> {
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("Enter an OpenAI API key to discover models.")?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|_| "The OpenAI API key contains invalid characters.")?;
    authorization.set_sensitive(true);

    if let Some(models) = cache.get(endpoint, api_key)? {
        return Ok(models);
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize OpenAI model discovery.")?;
    let response = client
        .get(endpoint)
        .header(AUTHORIZATION, authorization)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| "Could not reach the OpenAI Models API. Check your connection.")?;

    match response.status().as_u16() {
        401 | 403 => return Err("OpenAI rejected the API key or denied model access.".into()),
        429 => {
            return Err("OpenAI returned HTTP 429 (rate limit or quota). Try again later.".into())
        }
        status if !response.status().is_success() => {
            return Err(format!("OpenAI model discovery failed (HTTP {status})."));
        }
        _ => {}
    }

    let response: OpenAIApiResponse = response.json().await.map_err(|error| {
        if error.is_decode() {
            "OpenAI returned an invalid model list."
        } else {
            "Could not read the OpenAI model list. Check your connection."
        }
    })?;
    let mut seen = HashSet::new();
    let mut models: Vec<OpenAIModel> = response
        .data
        .into_iter()
        .filter(|model| is_chat_model(&model.id) && seen.insert(model.id.clone()))
        .map(|model| OpenAIModel { id: model.id })
        .collect();
    let recommended = recommended_models("openai");
    models.sort_by_key(|model| {
        recommended
            .iter()
            .position(|id| id == &model.id)
            .unwrap_or(usize::MAX)
    });
    cache.store(endpoint, api_key, models.clone())?;
    Ok(models)
}

pub fn clear_cache() {
    MODELS_CACHE.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_catalog::test_support::server;

    fn ids(models: Vec<OpenAIModel>) -> Vec<String> {
        models.into_iter().map(|model| model.id).collect()
    }

    #[tokio::test]
    async fn discovery_is_key_scoped_and_orders_only_discovered_text_models() {
        let (endpoint, requests, task) = server(|request| {
            if request.contains("Bearer key-b") {
                (200, r#"{"data":[{"id":"o3"}]}"#.into())
            } else {
                (200, r#"{"data":[{"id":"gpt-6.2-astra"},{"id":"gpt-6.2-sol"},{"id":"gpt-6-luna"},{"id":"gpt-4o-audio-preview"},{"id":"gpt-image-1"},{"id":"gpt-6-codex"},{"id":"o3-pro"},{"id":"gpt-5-pro-2025-10-06"},{"id":"o4-mini-deep-research"},{"id":"ft:gpt-4o:audio-lab:image"},{"id":"gpt-6-luna"}]}"#.into())
            }
        }).await;
        let cache = ModelCache::new();
        let first = || fetch_models(Some("key-a".into()), &endpoint, &cache);
        assert_eq!(
            ids(first().await.unwrap()),
            [
                "gpt-6-luna",
                "gpt-6.2-sol",
                "gpt-6.2-astra",
                "ft:gpt-4o:audio-lab:image"
            ]
        );
        assert_eq!(
            ids(first().await.unwrap()),
            [
                "gpt-6-luna",
                "gpt-6.2-sol",
                "gpt-6.2-astra",
                "ft:gpt-4o:audio-lab:image"
            ]
        );
        assert_eq!(
            ids(fetch_models(Some("key-b".into()), &endpoint, &cache)
                .await
                .unwrap()),
            ["o3"]
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        task.abort();
    }

    #[tokio::test]
    async fn discovery_distinguishes_empty_errors_and_retries_failures() {
        let (endpoint, requests, task) = server(|request| {
            if request.contains("Bearer bad") {
                (401, "{}".into())
            } else if request.contains("Bearer rate") {
                (429, "{}".into())
            } else if request.contains("Bearer malformed") {
                (200, "not json".into())
            } else {
                (200, r#"{"data":[]}"#.into())
            }
        })
        .await;
        let cache = ModelCache::new();
        assert!(fetch_models(None, &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("Enter"));
        assert!(fetch_models(Some("bad\nkey".into()), &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("invalid characters"));
        assert!(fetch_models(Some("bad".into()), &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("rejected"));
        assert!(fetch_models(Some("rate".into()), &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("HTTP 429"));
        assert!(fetch_models(Some("rate".into()), &endpoint, &cache)
            .await
            .is_err());
        assert!(fetch_models(Some("malformed".into()), &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("invalid model list"));
        assert!(fetch_models(Some("empty".into()), &endpoint, &cache)
            .await
            .unwrap()
            .is_empty());
        assert!(fetch_models(Some("empty".into()), &endpoint, &cache)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(requests.lock().unwrap().len(), 5);
        task.abort();
    }

    #[test]
    fn supports_existing_and_future_reasoning_ids() {
        for id in [
            "gpt-4",
            "o1",
            "o3",
            "o4-mini",
            "o5",
            "ft:gpt-4o:team:custom",
            "ft:gpt-4o:audio-lab:image",
            "gpt-5-prototype",
        ] {
            assert!(is_chat_model(id), "{id}");
        }
        for id in [
            "omni-moderation",
            "o3-audio",
            "gpt-4-instruct",
            "text-embedding-3-small",
            "ft:gpt-4o-audio-preview:org:custom",
            "o1-pro",
            "o1-pro-2025-03-19",
            "o3-pro",
            "o3-pro-2025-06-10",
            "gpt-5-pro",
            "gpt-5-pro-2025-10-06",
            "gpt-5.2-pro-2025-12-11",
            "gpt-5.4-pro-2026-03-05",
            "gpt-5.5-pro-2026-04-23",
            "o3-deep-research-2025-06-26",
            "o4-mini-deep-research-2025-06-26",
        ] {
            assert!(!is_chat_model(id), "{id}");
        }
    }

    #[tokio::test]
    async fn discovery_reports_network_errors() {
        let cache = ModelCache::new();
        assert!(
            fetch_models(Some("key".into()), "http://127.0.0.1:0/v1/models", &cache)
                .await
                .unwrap_err()
                .contains("Could not reach")
        );
    }
}
