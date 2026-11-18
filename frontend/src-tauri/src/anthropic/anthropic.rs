use crate::provider_catalog::{recommended_models, ModelCache};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tauri::command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicModel {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicApiModel {
    id: String,
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicApiResponse {
    data: Vec<AnthropicApiModel>,
    has_more: bool,
    last_id: Option<String>,
}

const MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const MAX_PAGES: usize = 100;
static MODELS_CACHE: ModelCache<AnthropicModel> = ModelCache::new();

#[command]
pub async fn get_anthropic_models(api_key: Option<String>) -> Result<Vec<AnthropicModel>, String> {
    fetch_models(api_key, MODELS_URL, &MODELS_CACHE).await
}

async fn fetch_models(
    api_key: Option<String>,
    endpoint: &str,
    cache: &ModelCache<AnthropicModel>,
) -> Result<Vec<AnthropicModel>, String> {
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("Enter an Anthropic API key to discover models.")?;
    let mut authorization = HeaderValue::from_str(api_key)
        .map_err(|_| "The Anthropic API key contains invalid characters.")?;
    authorization.set_sensitive(true);

    if let Some(models) = cache.get(endpoint, api_key)? {
        return Ok(models);
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize Anthropic model discovery.")?;
    let mut models = Vec::new();
    let mut seen_models = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut after_id: Option<String> = None;

    for page in 0..MAX_PAGES {
        let mut request = client
            .get(endpoint)
            .header("x-api-key", authorization.clone())
            .header("anthropic-version", "2023-06-01")
            .query(&[("limit", "1000")])
            .timeout(Duration::from_secs(5));
        if let Some(cursor) = &after_id {
            request = request.query(&[("after_id", cursor)]);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Could not reach the Anthropic Models API. Check your connection.")?;

        match response.status().as_u16() {
            401 | 403 => {
                return Err("Anthropic rejected the API key or denied model access.".into())
            }
            429 => {
                return Err(
                    "Anthropic returned HTTP 429 (rate limit or quota). Try again later.".into(),
                )
            }
            status if !response.status().is_success() => {
                return Err(format!("Anthropic model discovery failed (HTTP {status})."));
            }
            _ => {}
        }

        let response: AnthropicApiResponse = response.json().await.map_err(|error| {
            if error.is_decode() {
                "Anthropic returned an invalid model list."
            } else {
                "Could not read the Anthropic model list. Check your connection."
            }
        })?;
        let next_cursor = if response.has_more {
            let cursor = response
                .last_id
                .filter(|id| {
                    !id.is_empty() && response.data.last().is_some_and(|last| &last.id == id)
                })
                .ok_or("Anthropic returned an invalid model pagination cursor.")?;
            if !seen_cursors.insert(cursor.clone()) {
                return Err("Anthropic returned a repeated model pagination cursor.".into());
            }
            if page + 1 == MAX_PAGES {
                return Err("Anthropic model discovery exceeded the pagination limit.".into());
            }
            Some(cursor)
        } else {
            None
        };
        models.extend(response.data.into_iter().filter_map(|model| {
            (model.id.to_ascii_lowercase().starts_with("claude-")
                && seen_models.insert(model.id.clone()))
            .then_some(AnthropicModel {
                id: model.id,
                display_name: model.display_name,
            })
        }));
        match next_cursor {
            Some(cursor) => after_id = Some(cursor),
            None => {
                let recommended = recommended_models("claude");
                models.sort_by_key(|model| {
                    recommended
                        .iter()
                        .position(|id| id == &model.id)
                        .unwrap_or(usize::MAX)
                });
                cache.store(endpoint, api_key, models.clone())?;
                return Ok(models);
            }
        }
    }
    Err("Anthropic model discovery exceeded the pagination limit.".into())
}

pub fn clear_cache() {
    MODELS_CACHE.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_catalog::test_support::server;

    fn ids(models: Vec<AnthropicModel>) -> Vec<String> {
        models.into_iter().map(|model| model.id).collect()
    }

    #[tokio::test]
    async fn discovery_follows_cursors_and_caches_by_key() {
        let (endpoint, requests, task) = server(|request| {
            if request.contains("x-api-key: key-b") {
                (200, r#"{"data":[{"id":"claude-other","display_name":null}],"has_more":false,"last_id":"claude-other"}"#.into())
            } else if request.contains("after_id=claude-opus-5-5") {
                (200, r#"{"data":[{"id":"claude-sonnet-5-5","display_name":"Sonnet"},{"id":"unrelated","display_name":null}],"has_more":false,"last_id":"unrelated"}"#.into())
            } else {
                (200, r#"{"data":[{"id":"claude-haiku-5-5","display_name":"Haiku"},{"id":"claude-opus-5-5","display_name":"Opus"}],"has_more":true,"last_id":"claude-opus-5-5"}"#.into())
            }
        }).await;
        let cache = ModelCache::new();
        let first = || fetch_models(Some("key-a".into()), &endpoint, &cache);
        let models = first().await.unwrap();
        assert_eq!(models[0].display_name.as_deref(), Some("Sonnet"));
        assert_eq!(
            ids(models),
            ["claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-5-5"]
        );
        assert_eq!(
            ids(first().await.unwrap()),
            ["claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-5-5"]
        );
        assert_eq!(
            ids(fetch_models(Some("key-b".into()), &endpoint, &cache)
                .await
                .unwrap()),
            ["claude-other"]
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("after_id=claude-opus-5-5"));
        drop(requests);
        task.abort();
    }

    #[tokio::test]
    async fn discovery_distinguishes_empty_errors_and_retries_failures() {
        let (endpoint, requests, task) = server(|request| {
            if request.contains("x-api-key: bad") {
                (403, "{}".into())
            } else if request.contains("x-api-key: rate") {
                (429, "{}".into())
            } else if request.contains("x-api-key: malformed") {
                (200, "not json".into())
            } else {
                (200, r#"{"data":[],"has_more":false,"last_id":null}"#.into())
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

    #[tokio::test]
    async fn discovery_rejects_invalid_or_repeated_cursors() {
        for body in [
            r#"{"data":[],"has_more":true,"last_id":null}"#,
            r#"{"data":[{"id":"claude-one"}],"has_more":true,"last_id":"claude-wrong"}"#,
            r#"{"data":[{"id":"claude-one"}],"has_more":true,"last_id":"claude-one"}"#,
        ] {
            let (endpoint, _, task) = server(move |_| (200, body.into())).await;
            let cache = ModelCache::new();
            assert!(fetch_models(Some("key".into()), &endpoint, &cache)
                .await
                .unwrap_err()
                .contains("cursor"));
            task.abort();
        }
    }

    #[tokio::test]
    async fn discovery_stops_at_the_page_limit() {
        let (endpoint, requests, task) = server(|request| {
            let page = request
                .split("after_id=claude-")
                .nth(1)
                .and_then(|suffix| suffix.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|number| number.parse::<usize>().ok())
                .unwrap_or(0)
                + 1;
            let id = format!("claude-{page}");
            (
                200,
                format!(r#"{{"data":[{{"id":"{id}"}}],"has_more":true,"last_id":"{id}"}}"#),
            )
        })
        .await;
        let cache = ModelCache::new();
        assert!(fetch_models(Some("key".into()), &endpoint, &cache)
            .await
            .unwrap_err()
            .contains("pagination limit"));
        assert_eq!(requests.lock().unwrap().len(), MAX_PAGES);
        task.abort();
    }

    #[tokio::test]
    async fn discovery_reports_network_errors_and_does_not_cache_partial_pages() {
        let cache = ModelCache::new();
        assert!(
            fetch_models(Some("key".into()), "http://127.0.0.1:0/v1/models", &cache)
                .await
                .unwrap_err()
                .contains("Could not reach")
        );
        let (endpoint, requests, task) = server(|request| {
            if request.contains("after_id=claude-one") {
                (429, "{}".into())
            } else {
                (
                    200,
                    r#"{"data":[{"id":"claude-one"}],"has_more":true,"last_id":"claude-one"}"#
                        .into(),
                )
            }
        })
        .await;
        for _ in 0..2 {
            assert!(fetch_models(Some("key".into()), &endpoint, &cache)
                .await
                .unwrap_err()
                .contains("HTTP 429"));
        }
        assert_eq!(requests.lock().unwrap().len(), 4);
        task.abort();
    }
}
