use super::*;
use std::sync::{Arc, Mutex};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use tokio::net::TcpListener;

// Generated solely for these tests; it is not a credential for any service.
const N: &str = "r6QuqBKbXG-olt7Vjq872H0TZW2CU6VLof1KwnxQ5Pj34hibSM3hvxi7eNP3Q95X-v3FW1thMAy0GZoIC9BRCpmqPuWt1BvWSTlu23o9nc3vLEniyF4Sqj9lDEGR8HIjHMPxASypt9PrHAWw4VVZ-1YjgrUYGjn7ur0A0FfHJNX89rgD3EXlc5DI1yu0dOc47LkOjG1EQKwlGCkkfIjlU1yWoqoWZ5GxYYI36vBs5q-yJiaTm5rgVjzRoGTYVOw7UYRCyNvc5cuQaKOBG98wH2PWb14BIcs6BWFQYSHhvi0GmrDlswyLxMKHTfdW59zMvO_yoccxnyFR6c2H-KwUMQ";

fn keyset() -> KeySet {
    serde_json::from_value(json!({"keys": [{"kid":"test", "kty":"RSA", "alg":"RS256", "use":"sig", "n":N, "e":"AQAB"}]})).unwrap()
}

fn claims() -> Value {
    json!({"iss": ISSUER, "sub":"subject", "aud":"issued-client", "iat":now()-10,
        "exp":now()+300, "nonce":"nonce", "email":"person@example.test"})
}

fn signed(claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test".into());
    encode(&header, claims, &EncodingKey::from_rsa_der(include_bytes!("test_key_only.der"))).unwrap()
}

fn registration() -> Registration {
    Registration { client_id: "issued-client".into(), sub: "subject".into(), email: Some("person@example.test".into()) }
}

fn grant() -> Grant {
    Grant { registration: registration(), access_token: "old-access".into(), refresh_token: "old-refresh".into(),
        id_token: "previous-id".into(), expires_at: now()+100, earliest_refresh_at: None,
        scopes: vec!["offline_access".into(), "chatgpt.tokens.use.direct".into()] }
}

fn provider(base: &str) -> Provider {
    Provider { issuer: ISSUER.into(), authorization_endpoint: format!("{base}/authorize"), token_endpoint: format!("{base}/token"),
        revocation_endpoint: format!("{base}/revoke"), jwks_uri: format!("{base}/jwks") }
}

#[test]
fn authorization_uses_pkce_and_distinct_registration_requests() {
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let provider = provider("https://auth.openai.com");
    let first = authorization_url(&provider, "urn:uuid:host", None, "http://127.0.0.1:123/auth/callback", "state", "nonce", "verifier", false).unwrap();
    let query: std::collections::HashMap<_, _> = first.query_pairs().into_owned().collect();
    assert_eq!(query.get("client_id").unwrap(), DYNAMIC_CLIENT);
    assert_eq!(query.get("agent_name_hint").unwrap(), "Meetily Sapphire");
    assert_eq!(query.get("ext_agent_host_id").unwrap(), "urn:uuid:host");
    assert_eq!(query.get("resource").unwrap(), RESOURCE);
    assert_eq!(query.get("scope").unwrap(), SCOPES);
    assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(query.get("redirect_uri").unwrap(), "http://127.0.0.1:123/auth/callback");
    let returning = authorization_url(&provider, "host", Some(&registration()), "http://127.0.0.1:321/auth/callback", "s", "n", "v", false).unwrap();
    let query: std::collections::HashMap<_, _> = returning.query_pairs().into_owned().collect();
    assert_eq!(query.get("client_id").unwrap(), "issued-client");
    assert_eq!(query.get("login_hint").unwrap(), "person@example.test");
    assert!(!query.contains_key("agent_name_hint"));
    assert!(!query.contains_key("prompt"));
    let consent = authorization_url(&provider, "host", Some(&registration()), "http://127.0.0.1:321/auth/callback", "s", "n", "v", true).unwrap();
    let query: std::collections::HashMap<_, _> = consent.query_pairs().into_owned().collect();
    assert_eq!(query.get("prompt").unwrap(), "consent");
    assert_eq!(query.get("scope").unwrap(), SCOPES);
    assert_ne!(random_value(), random_value());
}

fn request(target: &str, host: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n")
}

#[test]
fn callback_rejects_unbound_malformed_and_mismatched_results() {
    let host = "127.0.0.1:123";
    let parse = |target: &str| parse_callback(&request(target, host), host, "expected", None);
    assert!(parse("/auth/callback?state=wrong&code=abc&client_id=issued-client").is_none());
    assert!(parse("/auth/callback?state=expected&state=expected&code=abc&client_id=issued-client").is_none());
    assert!(parse("/callback?state=expected&code=abc&client_id=issued-client").is_none());
    assert!(parse_callback(&request("/auth/callback?state=expected", "localhost:123"), host, "expected", None).is_none());
    assert!(parse("/auth/callback?state=expected&code=abc").unwrap().is_err());
    assert!(parse("/auth/callback?state=expected&code=abc&client_id=dynamic_agent_client").unwrap().is_err());
    assert!(parse("/auth/callback?state=expected&code=abc&code=def&client_id=issued-client").unwrap().is_err());
    assert!(parse("/auth/callback?state=expected&error=access_denied&error_description=private").unwrap().err().unwrap().contains("declined"));
    let existing = registration();
    let mismatched = request("/auth/callback?state=expected&code=abc&client_id=another-client", host);
    assert!(parse_callback(&mismatched, host, "expected", Some(&existing)).unwrap().is_err());
    let returning = request("/auth/callback?state=expected&code=abc", host);
    assert_eq!(parse_callback(&returning, host, "expected", Some(&existing)).unwrap().ok().unwrap().client_id, "issued-client");
}

#[test]
fn identity_requires_signature_issuer_audience_nonce_and_expiry() {
    let check = |value: &Value| verify_identity(&signed(value), "issued-client", Some("nonce"), "access", now(), &keyset()).is_ok();
    let value = claims();
    assert!(check(&value));
    for (field, invalid) in [("iss", json!("https://elsewhere.test")), ("aud", json!("other-client")),
        ("nonce", json!("wrong")), ("exp", json!(now()-60)), ("iat", json!(now()+60)), ("nbf", json!(now()+60)),
        ("sub", json!("")), ("aud", json!(["issued-client", "other-client"]))] {
        let mut broken = value.clone();
        broken[field] = invalid;
        assert!(!check(&broken), "accepted invalid {field}");
    }
    let mut multi = value.clone();
    multi["aud"] = json!(["issued-client", "other-client"]);
    multi["azp"] = json!("issued-client");
    assert!(check(&multi));
    let mut tampered = signed(&value).into_bytes();
    let last = tampered.len()-1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    assert!(verify_identity(std::str::from_utf8(&tampered).unwrap(), "issued-client", Some("nonce"), "access", now(), &keyset()).is_err());
    let mut with_hash = value.clone();
    let digest = Sha256::digest(b"access");
    with_hash["at_hash"] = json!(URL_SAFE_NO_PAD.encode(&digest[..16]));
    assert!(check(&with_hash));
    with_hash["at_hash"] = json!("wrong");
    assert!(!check(&with_hash));
}

#[test]
fn token_fields_retain_missing_refresh_scopes_and_parse_refresh_time() {
    let response: TokenResponse = serde_json::from_value(json!({"access_token":"a", "refresh_token":"r", "token_type":"Bearer", "expires_in":60, "earliest_refresh_at":"2030-01-01T00:00:00Z"})).unwrap();
    let (pending, _) = token_fields(response, Some(&["chatgpt.tokens.use.direct".into()]), true).unwrap();
    assert_eq!(pending.earliest_refresh_at, Some(1893456000));
    assert_eq!(pending.scopes, ["chatgpt.tokens.use.direct"]);
    let invalid: TokenResponse = serde_json::from_value(json!({"access_token":"a", "token_type":"Bearer", "expires_in":60, "scope":"offline_access"})).unwrap();
    assert!(token_fields(invalid, None, false).is_err());
}

async fn server(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let size = stream.read(&mut chunk).await.unwrap();
                if size == 0 { break; }
                bytes.extend_from_slice(&chunk[..size]);
                if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                    }).unwrap_or(0);
                    if bytes.len() >= header_end + 4 + length { break; }
                }
            }
            captured.lock().unwrap().push(String::from_utf8(bytes).unwrap());
            let reply = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (base, requests, task)
}

#[tokio::test]
async fn rotated_refresh_is_checkpointed_before_identity_recovery() {
    let jwt = signed(&claims());
    let body = json!({"access_token":"new-access", "refresh_token":"new-refresh", "id_token":jwt,
        "token_type":"Bearer", "expires_in":3600}).to_string();
    let (base, requests, task) = server(vec![(200, body), (503, "{}".into())]).await;
    let saved = Arc::new(Mutex::new(None));
    let checkpoint = saved.clone();
    let result = refresh_with_provider(&reqwest::Client::new(), &provider(&base), &grant(), move |pending| async move {
        *checkpoint.lock().unwrap() = Some(pending);
        Ok(())
    }).await;
    assert!(result.is_err());
    task.await.unwrap();
    let pending = saved.lock().unwrap().take().expect("rotation must be checkpointed before JWKS");
    assert_eq!(pending.refresh_token, "new-refresh");
    let captured = requests.lock().unwrap();
    let body = captured[0].split("\r\n\r\n").nth(1).unwrap();
    let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
    assert_eq!(fields.get("grant_type").unwrap(), "refresh_token");
    assert_eq!(fields.get("client_id").unwrap(), "issued-client");
    assert_eq!(fields.get("refresh_token").unwrap(), "old-refresh");
    assert_eq!(fields.get("resource").unwrap(), RESOURCE);
    assert!(!fields.contains_key("scope"));
    drop(captured);
    let jwks = json!({"keys": [{"kid":"test", "kty":"RSA", "n":N, "e":"AQAB"}]}).to_string();
    let (base, _, task) = server(vec![(200, jwks)]).await;
    let recovered = finish_refresh(&reqwest::Client::new(), &provider(&base), &grant(), &pending).await.unwrap();
    task.await.unwrap();
    assert_eq!(recovered.refresh_token, "new-refresh");
    assert_eq!(recovered.registration.sub, "subject");
}

#[tokio::test]
async fn revocation_retries_and_uses_the_replacement_token() {
    let (base, requests, task) = server(vec![(503, "{}".into()), (200, "".into())]).await;
    revoke_with_provider(&reqwest::Client::new(), &provider(&base), "issued-client", "replacement-refresh").await.unwrap();
    task.await.unwrap();
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    for request in captured.iter() {
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
        assert_eq!(fields.get("token").unwrap(), "replacement-refresh");
        assert_eq!(fields.get("token_type_hint").unwrap(), "refresh_token");
        assert_eq!(fields.get("client_id").unwrap(), "issued-client");
    }
}

#[tokio::test]
async fn cancelled_sign_in_never_starts_network_or_browser() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = authorize(&reqwest::Client::new(), "host", None, &cancel, false, |_| async { Ok(()) }).await;
    assert_eq!(result.err().unwrap(), "ChatGPT sign-in was cancelled.");
}

#[tokio::test]
async fn cancellation_and_timeout_interrupt_loopback_wait() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let sender = tokio::spawn(async move {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(b"GET /auth/callback HTTP/1.1\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        trigger.cancel();
    });
    let result = cancellable(&cancel, Duration::from_secs(2), wait_callback(listener, "state", None)).await;
    assert_eq!(result.err().unwrap(), "ChatGPT sign-in was cancelled.");
    sender.await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let error = cancellable(&CancellationToken::new(), Duration::from_millis(20), wait_callback(listener, "state", None)).await.err().unwrap();
    assert!(error.contains("timed out"));
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_token_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let _ = stream.read(&mut buffer).await.unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n{").await.unwrap();
        let _ = started_tx.send(());
        let _ = release_rx.await;
    });
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let sender = tokio::spawn(async move { started_rx.await.unwrap(); trigger.cancel(); });
    let client = reqwest::Client::new();
    let result = cancellable(&cancel, Duration::from_secs(2), post_form(&client, &endpoint, &[("code", "synthetic")])).await;
    assert_eq!(result.err().unwrap(), "ChatGPT sign-in was cancelled.");
    sender.await.unwrap();
    let _ = release_tx.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn oauth_errors_do_not_disclose_response_descriptions() {
    let (base, _, task) = server(vec![(400, json!({"error":"invalid_grant", "error_description":"synthetic-secret"}).to_string())]).await;
    let error = post_form(&reqwest::Client::new(), &format!("{base}/token"), &[("code", "synthetic")]).await.err().unwrap();
    task.await.unwrap();
    assert_eq!(error, SESSION_EXPIRED);
    assert!(!error.contains("synthetic-secret"));
}
