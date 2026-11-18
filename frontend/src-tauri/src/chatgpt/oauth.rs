//! Native Sign in with ChatGPT authorization. Tokens never leave the native backend.

use std::{future::Future, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
use tokio_util::sync::CancellationToken;
use url::Url;

const ISSUER: &str = "https://auth.openai.com";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const CALLBACK_PATH: &str = "/auth/callback";
const INVALID_IDENTITY: &str = "The ChatGPT identity could not be verified. Please sign in again.";
const INVALID_RESPONSE: &str = "ChatGPT returned an invalid token response. Please sign in again.";
const NETWORK_ERROR: &str = "ChatGPT could not be reached. Check your connection and try again.";
const SESSION_EXPIRED: &str = "ChatGPT session expired or was revoked; sign in again.";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Registration {
    pub(super) client_id: String,
    pub(super) sub: String,
    pub(super) email: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Grant {
    pub(super) registration: Registration,
    pub(super) access_token: String,
    pub(super) refresh_token: String,
    pub(super) id_token: String,
    pub(super) expires_at: i64,
    pub(super) earliest_refresh_at: Option<i64>,
    pub(super) scopes: Vec<String>,
}

// An encrypted checkpoint only: it must be verified before becoming an active Grant.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingRefresh {
    pub(super) access_token: String,
    pub(super) refresh_token: String,
    pub(super) id_token: Option<String>,
    pub(super) expires_at: i64,
    pub(super) earliest_refresh_at: Option<i64>,
    pub(super) scopes: Vec<String>,
    pub(super) received_at: i64,
}

#[derive(Deserialize)]
struct Provider {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Key>,
}

#[derive(Deserialize)]
struct Key {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    alg: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    iat: i64,
    exp: i64,
    nbf: Option<i64>,
    aud: serde_json::Value,
    nonce: Option<String>,
    azp: Option<String>,
    email: Option<String>,
    at_hash: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    earliest_refresh_at: Option<serde_json::Value>,
}

struct Callback {
    code: String,
    client_id: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn random_value() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn equal_secret(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left.bytes().zip(right.bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn valid_client_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id != DYNAMIC_CLIENT
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

async fn read_json<T: for<'de> Deserialize<'de>>(mut response: reqwest::Response) -> Result<T, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| NETWORK_ERROR.to_string())? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err(INVALID_RESPONSE.to_string());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| INVALID_RESPONSE.to_string())
}

fn trusted_endpoint(raw: &str) -> bool {
    Url::parse(raw).ok().is_some_and(|url| {
        url.scheme() == "https" && url.host_str() == Some("auth.openai.com")
            && url.port().is_none() && url.username().is_empty() && url.password().is_none()
            && url.query().is_none() && url.fragment().is_none()
    })
}

async fn discovery(client: &reqwest::Client) -> Result<Provider, String> {
    let response = client.get(DISCOVERY).send().await.map_err(|_| NETWORK_ERROR.to_string())?;
    if !response.status().is_success() || response.url().as_str() != DISCOVERY {
        return Err("ChatGPT sign-in configuration could not be verified.".into());
    }
    let provider: Provider = read_json(response).await?;
    if provider.issuer != ISSUER || ![
        &provider.authorization_endpoint, &provider.token_endpoint,
        &provider.revocation_endpoint, &provider.jwks_uri,
    ].into_iter().all(|endpoint| trusted_endpoint(endpoint)) {
        return Err("ChatGPT sign-in configuration could not be verified.".into());
    }
    Ok(provider)
}

async fn keys(client: &reqwest::Client, provider: &Provider) -> Result<KeySet, String> {
    let response = client.get(&provider.jwks_uri).send().await.map_err(|_| NETWORK_ERROR.to_string())?;
    if !response.status().is_success() || response.url().as_str() != provider.jwks_uri {
        return Err("ChatGPT identity verification is temporarily unavailable.".into());
    }
    read_json(response).await.map_err(|_| "ChatGPT identity verification is temporarily unavailable.".into())
}

fn verify_identity(token: &str, client_id: &str, nonce: Option<&str>, access_token: &str, received_at: i64, keys: &KeySet) -> Result<Claims, String> {
    let invalid = || INVALID_IDENTITY.to_string();
    let header = decode_header(token).map_err(|_| invalid())?;
    if header.alg != Algorithm::RS256 { return Err(invalid()); }
    let kid = header.kid.ok_or_else(invalid)?;
    let mut candidates = keys.keys.iter().filter(|key| {
        key.kid.as_deref() == Some(kid.as_str()) && key.kty == "RSA"
            && key.alg.as_deref().map_or(true, |alg| alg == "RS256")
            && key.usage.as_deref().map_or(true, |usage| usage == "sig")
    });
    let key = candidates.next().ok_or_else(invalid)?;
    if candidates.next().is_some() { return Err(invalid()); }
    let decoding = DecodingKey::from_rsa_components(
        key.n.as_deref().ok_or_else(invalid)?, key.e.as_deref().ok_or_else(invalid)?,
    ).map_err(|_| invalid())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "iat", "sub"]);
    // Pending refreshes are checked at receipt so they can recover after an outage.
    validation.validate_exp = false;
    let claims = decode::<Claims>(token, &decoding, &validation).map_err(|_| invalid())?.claims;
    if received_at > now().saturating_add(5) || claims.iss != ISSUER || claims.sub.trim().is_empty()
        || claims.exp <= received_at.saturating_sub(5) || claims.iat > received_at.saturating_add(5)
        || claims.nbf.is_some_and(|nbf| nbf > received_at.saturating_add(5))
        || nonce.is_some_and(|nonce| claims.nonce.as_deref() != Some(nonce))
        || claims.azp.as_deref().is_some_and(|azp| azp != client_id)
        || claims.aud.as_array().is_some_and(|aud| aud.len() > 1 && claims.azp.as_deref() != Some(client_id))
    { return Err(invalid()); }
    if let Some(at_hash) = &claims.at_hash {
        let digest = Sha256::digest(access_token.as_bytes());
        if !equal_secret(at_hash, &URL_SAFE_NO_PAD.encode(&digest[..16])) { return Err(invalid()); }
    }
    Ok(claims)
}

fn earliest(value: Option<serde_json::Value>) -> Result<Option<i64>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => number.as_i64().map(Some).ok_or_else(|| INVALID_RESPONSE.into()),
        Some(serde_json::Value::String(value)) => value.parse::<i64>().ok().or_else(|| {
            chrono::DateTime::parse_from_rfc3339(&value).ok().map(|date| date.timestamp())
        }).map(Some).ok_or_else(|| INVALID_RESPONSE.into()),
        _ => Err(INVALID_RESPONSE.into()),
    }
}

fn token_fields(response: TokenResponse, previous_scopes: Option<&[String]>, require_refresh: bool) -> Result<(PendingRefresh, Option<String>), String> {
    let received_at = now();
    let scopes = match response.scope {
        Some(scope) => scope.split_whitespace().map(str::to_owned).collect::<Vec<_>>(),
        None => previous_scopes.ok_or_else(|| INVALID_RESPONSE.to_string())?.to_vec(),
    };
    let access = response.access_token.unwrap_or_default();
    let refresh = response.refresh_token.unwrap_or_default();
    let needs_access = require_refresh || scopes.iter().any(|scope| scope == "offline_access" || scope == "chatgpt.tokens.use.direct");
    let needs_refresh = require_refresh || scopes.iter().any(|scope| scope == "offline_access");
    if (needs_access && access.is_empty()) || (needs_refresh && refresh.is_empty())
        || response.id_token.as_ref().is_some_and(|value| value.is_empty())
    { return Err(INVALID_RESPONSE.into()); }
    let expires_at = if access.is_empty() {
        0
    } else {
        let expires = response.expires_in.filter(|value| *value > 0).ok_or_else(|| INVALID_RESPONSE.to_string())?;
        if !response.token_type.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("bearer")) {
            return Err(INVALID_RESPONSE.into());
        }
        received_at.checked_add(expires).ok_or_else(|| INVALID_RESPONSE.to_string())?
    };
    let identity = response.id_token;
    Ok((PendingRefresh {
        access_token: access, refresh_token: refresh, id_token: identity.clone(), expires_at,
        earliest_refresh_at: earliest(response.earliest_refresh_at)?, scopes, received_at,
    }, identity))
}

async fn post_form(client: &reqwest::Client, endpoint: &str, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let response = client.post(endpoint).form(form).send().await.map_err(|_| NETWORK_ERROR.to_string())?;
    if response.url().as_str() != endpoint { return Err(NETWORK_ERROR.into()); }
    if !response.status().is_success() {
        let status = response.status();
        let error: serde_json::Value = read_json(response).await.unwrap_or(serde_json::Value::Null);
        let code = error.get("error").and_then(|error| error.as_str()).unwrap_or("");
        let reason = error.get("error_reason").and_then(|error| error.as_str()).unwrap_or("");
        if ["invalid_grant", "invalid_refresh_token", "token_expired", "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"].contains(&code)
            || ["invalid_refresh_token", "token_expired", "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"].contains(&reason)
        { return Err(SESSION_EXPIRED.into()); }
        if code == "invalid_client" { return Err("ChatGPT registration is invalid; sign in again.".into()); }
        return Err(format!("ChatGPT authorization failed (HTTP {}). Please try again.", status.as_u16()));
    }
    read_json(response).await
}

fn authorization_url(provider: &Provider, host_id: &str, previous: Option<&Registration>, redirect_uri: &str, state: &str, nonce: &str, verifier: &str, reconsent: bool) -> Result<Url, String> {
    if host_id.trim().is_empty() || previous.is_some_and(|entry| !valid_client_id(&entry.client_id)) {
        return Err("The ChatGPT registration is invalid.".into());
    }
    let mut url = Url::parse(&provider.authorization_endpoint).map_err(|_| "ChatGPT sign-in configuration could not be verified.".to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("client_id", previous.map_or(DYNAMIC_CLIENT, |entry| entry.client_id.as_str()));
        pairs.append_pair("ext_agent_host_id", host_id);
        if let Some(registration) = previous {
            if let Some(email) = registration.email.as_ref().filter(|value| !value.is_empty()) { pairs.append_pair("login_hint", email); }
            if reconsent { pairs.append_pair("prompt", "consent"); }
        } else {
            pairs.append_pair("agent_name_hint", "Meetily Sapphire");
        }
        pairs.append_pair("response_type", "code").append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", SCOPES).append_pair("resource", RESOURCE)
            .append_pair("state", state).append_pair("nonce", nonce)
            .append_pair("code_challenge_method", "S256").append_pair("code_challenge", &pkce_challenge(verifier));
    }
    Ok(url)
}

// None means this request did not authenticate the pending browser transaction.
fn parse_callback(request: &str, expected_host: &str, state: &str, previous: Option<&Registration>) -> Option<Result<Callback, String>> {
    let mut lines = request.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let (method, target, version) = (first.next()?, first.next()?, first.next()?);
    if method != "GET" || !matches!(version, "HTTP/1.1" | "HTTP/1.0") || first.next().is_some() { return None; }
    let mut hosts = lines.take_while(|line| !line.is_empty()).filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host").then(|| value.trim())
    });
    if hosts.next() != Some(expected_host) || hosts.next().is_some() { return None; }
    let (path, query) = target.split_once('?')?;
    if path != CALLBACK_PATH { return None; }
    let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let values = |name: &str| params.iter().filter(move |(key, _)| key == name).map(|(_, value)| value.as_str()).collect::<Vec<_>>();
    let states = values("state");
    if states.len() != 1 || !equal_secret(states[0], state) { return None; }
    let errors = values("error");
    if !errors.is_empty() {
        return Some(Err(if errors.len() == 1 && errors[0] == "access_denied" {
            "ChatGPT sign-in was declined.".into()
        } else { "ChatGPT sign-in could not be completed.".into() }));
    }
    let codes = values("code");
    let clients = values("client_id");
    let client_id = clients.first().copied().or_else(|| previous.map(|entry| entry.client_id.as_str()));
    if codes.len() != 1 || codes[0].is_empty() || clients.len() > 1 || client_id.map_or(true, |id| !valid_client_id(id))
        || previous.is_some_and(|entry| clients.first().is_some_and(|client| *client != entry.client_id))
    { return Some(Err("ChatGPT did not complete app registration. Please try signing in again.".into())); }
    Some(Ok(Callback { code: codes[0].to_owned(), client_id: client_id.unwrap().to_owned() }))
}

async fn wait_callback(listener: TcpListener, state: &str, previous: Option<&Registration>) -> Result<Callback, String> {
    let port = listener.local_addr().map_err(|_| "The local sign-in listener stopped.".to_string())?.port();
    let expected_host = format!("127.0.0.1:{port}");
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|_| "The local sign-in listener stopped.".to_string())?;
        let request = tokio::time::timeout(Duration::from_secs(10), async {
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 1024];
            while bytes.len() < 16384 {
                let size = stream.read(&mut buffer).await.ok()?;
                if size == 0 { return None; }
                bytes.extend_from_slice(&buffer[..size]);
                if bytes.windows(4).any(|part| part == b"\r\n\r\n") { return String::from_utf8(bytes).ok(); }
            }
            None
        }).await.ok().flatten();
        let result = request.as_deref().and_then(|request| parse_callback(request, &expected_host, state, previous));
        let body = if result.is_some() { "Return to Meetily. You can close this tab." } else { "Invalid sign-in request." };
        let status = if result.is_some() { "200 OK" } else { "400 Bad Request" };
        let reply = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = tokio::time::timeout(Duration::from_secs(2), stream.write_all(reply.as_bytes())).await;
        if let Some(result) = result { return result; }
    }
}

async fn cancellable<T>(cancel: &CancellationToken, timeout: Duration, future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("ChatGPT sign-in was cancelled.".into()),
        result = tokio::time::timeout(timeout, future) => result.unwrap_or_else(|_| Err("ChatGPT sign-in timed out. Please try again.".into())),
    }
}

pub(super) async fn authorize<F, Fut>(client: &reqwest::Client, host_id: &str, previous: Option<&Registration>, cancel: &CancellationToken, reconsent: bool, on_registration: F) -> Result<Grant, String>
where F: FnOnce(String) -> Fut, Fut: Future<Output = Result<(), String>> {
    let start = async {
        let provider = discovery(client).await?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| "The local sign-in port is unavailable.".to_string())?;
        let port = listener.local_addr().map_err(|_| "The local sign-in listener stopped.".to_string())?.port();
        let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let (state, nonce, verifier) = (random_value(), random_value(), random_value());
        let url = authorization_url(&provider, host_id, previous, &redirect, &state, &nonce, &verifier, reconsent)?.to_string();
        tokio::task::spawn_blocking(move || {
            #[cfg(windows)]
            let result = open::that_detached(url);
            #[cfg(not(windows))]
            let result = open::that(url);
            result.map_err(|_| "The browser could not be opened. Check your default browser and try again.".to_string())
        }).await.map_err(|_| "The browser could not be opened. Check your default browser and try again.".to_string())??;
        let callback = wait_callback(listener, &state, previous).await?;
        Ok::<_, String>((provider, callback, redirect, nonce, verifier))
    };
    let (provider, callback, redirect, nonce, verifier) = cancellable(cancel, Duration::from_secs(600), start).await?;
    // Save the issued registration even if cancellation arrives during this write.
    on_registration(callback.client_id.clone()).await?;
    let finish = async {
        let response = post_form(client, &provider.token_endpoint, &[
            ("grant_type", "authorization_code"), ("client_id", &callback.client_id), ("code", &callback.code),
            ("code_verifier", &verifier), ("redirect_uri", &redirect), ("resource", RESOURCE),
        ]).await?;
        let (fields, identity) = token_fields(response, None, false)?;
        let identity = identity.ok_or_else(|| INVALID_RESPONSE.to_string())?;
        let jwks = keys(client, &provider).await?;
        let claims = verify_identity(&identity, &callback.client_id, Some(&nonce), &fields.access_token, fields.received_at, &jwks)?;
        if previous.is_some_and(|entry| !entry.sub.is_empty() && entry.sub != claims.sub) {
            return Err("This sign-in returned a different ChatGPT account. Choose the original account or add a separate account.".into());
        }
        Ok(Grant {
            registration: Registration { client_id: callback.client_id, sub: claims.sub, email: claims.email.filter(|email| !email.is_empty()) },
            access_token: fields.access_token, refresh_token: fields.refresh_token, id_token: identity,
            expires_at: fields.expires_at, earliest_refresh_at: fields.earliest_refresh_at, scopes: fields.scopes,
        })
    };
    cancellable(cancel, Duration::from_secs(60), finish).await
}

pub(super) async fn refresh<F, Fut>(client: &reqwest::Client, grant: &Grant, checkpoint: F) -> Result<Grant, String>
where F: FnOnce(PendingRefresh) -> Fut, Fut: Future<Output = Result<(), String>> {
    if grant.refresh_token.is_empty() || !valid_client_id(&grant.registration.client_id) { return Err(SESSION_EXPIRED.into()); }
    let provider = tokio::time::timeout(Duration::from_secs(30), discovery(client)).await.map_err(|_| NETWORK_ERROR.to_string())??;
    refresh_with_provider(client, &provider, grant, checkpoint).await
}

async fn refresh_with_provider<F, Fut>(client: &reqwest::Client, provider: &Provider, grant: &Grant, checkpoint: F) -> Result<Grant, String>
where F: FnOnce(PendingRefresh) -> Fut, Fut: Future<Output = Result<(), String>> {
        let response = tokio::time::timeout(Duration::from_secs(30), async {
        let response = post_form(client, &provider.token_endpoint, &[
            ("grant_type", "refresh_token"), ("client_id", &grant.registration.client_id),
            ("refresh_token", &grant.refresh_token), ("resource", RESOURCE),
        ]).await?;
        Ok::<_, String>(response)
        }).await.map_err(|_| NETWORK_ERROR.to_string())??;
        let (pending, _) = token_fields(response, Some(&grant.scopes), true)?;
        checkpoint(pending.clone()).await?;
        tokio::time::timeout(Duration::from_secs(30), finish_refresh(client, provider, grant, &pending)).await.unwrap_or_else(|_| Err(NETWORK_ERROR.into()))
}

pub(super) async fn resume_refresh(client: &reqwest::Client, grant: &Grant, pending: &PendingRefresh) -> Result<Grant, String> {
    let operation = async {
        let provider = discovery(client).await?;
        finish_refresh(client, &provider, grant, pending).await
    };
    tokio::time::timeout(Duration::from_secs(60), operation).await.unwrap_or_else(|_| Err(NETWORK_ERROR.into()))
}

async fn finish_refresh(client: &reqwest::Client, provider: &Provider, grant: &Grant, pending: &PendingRefresh) -> Result<Grant, String> {
    let mut registration = grant.registration.clone();
    let id_token = if let Some(identity) = &pending.id_token {
        let jwks = keys(client, provider).await?;
        let claims = verify_identity(identity, &registration.client_id, None, &pending.access_token, pending.received_at, &jwks)?;
        if claims.sub != registration.sub { return Err("The refreshed ChatGPT identity does not match this account. Sign in again.".into()); }
        registration.email = claims.email.filter(|email| !email.is_empty());
        identity.clone()
    } else { grant.id_token.clone() };
    Ok(Grant {
        registration, access_token: pending.access_token.clone(), refresh_token: pending.refresh_token.clone(), id_token,
        expires_at: pending.expires_at, earliest_refresh_at: pending.earliest_refresh_at, scopes: pending.scopes.clone(),
    })
}

pub(super) async fn revoke(client: &reqwest::Client, grant: &Grant) -> Result<(), String> {
    revoke_token(client, &grant.registration.client_id, &grant.refresh_token).await
}

pub(super) async fn revoke_pending(client: &reqwest::Client, grant: &Grant, pending: &PendingRefresh) -> Result<(), String> {
    revoke_token(client, &grant.registration.client_id, &pending.refresh_token).await
}

async fn revoke_token(client: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<(), String> {
    let operation = async {
        if refresh_token.is_empty() { return Ok(()); }
        let provider = discovery(client).await?;
        revoke_with_provider(client, &provider, client_id, refresh_token).await
    };
    tokio::time::timeout(Duration::from_secs(30), operation).await.unwrap_or_else(|_| Err("ChatGPT remote disconnection could not be confirmed.".into()))
}

async fn revoke_with_provider(client: &reqwest::Client, provider: &Provider, client_id: &str, refresh_token: &str) -> Result<(), String> {
        for attempt in 0..2 {
            let response = client.post(&provider.revocation_endpoint).form(&[
                ("token", refresh_token), ("token_type_hint", "refresh_token"),
                ("client_id", client_id),
            ]).timeout(Duration::from_secs(10)).send().await;
            match response {
                Ok(response) if response.status().as_u16() == 200 && response.url().as_str() == provider.revocation_endpoint => return Ok(()),
                Ok(response) if !response.status().is_server_error() => break,
                _ => (),
            }
            if attempt == 0 { tokio::time::sleep(Duration::from_millis(300)).await; }
        }
        Err("ChatGPT remote disconnection could not be confirmed.".to_string())
}

#[cfg(test)]
#[path = "oauth/tests.rs"]
mod tests;
