use once_cell::sync::Lazy;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::RwLock;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct HostedModelCatalog {
    openai: Vec<String>,
    claude: Vec<String>,
}

static HOSTED_MODELS: Lazy<HostedModelCatalog> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../src/constants/hostedModelCatalog.json"))
        .expect("bundled hosted model catalog must be valid")
});

pub(crate) fn recommended_models(provider: &str) -> &'static [String] {
    match provider {
        "openai" => &HOSTED_MODELS.openai,
        "claude" => &HOSTED_MODELS.claude,
        _ => &[],
    }
}

struct CacheEntry<T> {
    credential: [u8; 32],
    models: Vec<T>,
    fetched_at: Instant,
}

pub(crate) struct ModelCache<T>(RwLock<Option<CacheEntry<T>>>);

impl<T: Clone> ModelCache<T> {
    pub(crate) const fn new() -> Self {
        Self(RwLock::new(None))
    }

    fn fingerprint(endpoint: &str, api_key: &str) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(endpoint.as_bytes());
        hash.update([0]);
        hash.update(api_key.as_bytes());
        hash.finalize().into()
    }

    pub(crate) fn get(&self, endpoint: &str, api_key: &str) -> Result<Option<Vec<T>>, String> {
        let cache = self.0.read().map_err(|_| "Model cache unavailable")?;
        Ok(cache.as_ref().and_then(|entry| {
            (entry.credential == Self::fingerprint(endpoint, api_key)
                && entry.fetched_at.elapsed() < Duration::from_secs(300))
            .then(|| entry.models.clone())
        }))
    }

    pub(crate) fn store(
        &self,
        endpoint: &str,
        api_key: &str,
        models: Vec<T>,
    ) -> Result<(), String> {
        let mut cache = self.0.write().map_err(|_| "Model cache unavailable")?;
        *cache = Some(CacheEntry {
            credential: Self::fingerprint(endpoint, api_key),
            models,
            fetched_at: Instant::now(),
        });
        Ok(())
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut cache) = self.0.write() {
            *cache = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_expires_and_is_scoped_to_the_key_and_endpoint() {
        let cache = ModelCache::new();
        cache.store("endpoint-a", "key-a", vec!["model"]).unwrap();
        assert_eq!(
            cache.get("endpoint-a", "key-a").unwrap(),
            Some(vec!["model"])
        );
        assert_eq!(cache.get("endpoint-a", "key-b").unwrap(), None);
        assert_eq!(cache.get("endpoint-b", "key-a").unwrap(), None);
        cache.0.write().unwrap().as_mut().unwrap().fetched_at =
            Instant::now() - Duration::from_secs(301);
        assert_eq!(cache.get("endpoint-a", "key-a").unwrap(), None);
        cache.clear();
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub(crate) async fn server(
        handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/models", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut chunk = [0; 2048];
                loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    if request.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                captured.lock().unwrap().push(request.clone());
                let (status, body) = handler(&request);
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (endpoint, requests, task)
    }
}
