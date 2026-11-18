use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const MODELS_URL: &str = "https://api.openai.com/v1/models";
const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CANCELLED: &str = "Summary generation was cancelled";

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanModel {
    pub slug: String,
    pub display_name: String,
}

async fn or_cancel<T>(
    cancel: &CancellationToken,
    operation: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CANCELLED.to_string()),
        result = operation => Ok(result),
    }
}

// Only retain machine-readable identifiers, never server messages that could echo private input.
fn safe_field<'a>(value: Option<&'a str>, access_token: &str) -> Option<&'a str> {
    value.filter(|s| {
        !s.is_empty()
            && (access_token.is_empty() || !s.contains(access_token))
            && s.len() <= 160
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']'))
    })
}

fn recovery(code: &str) -> Option<&'static str> {
    match code {
        "subscription_sharing_usage_limit_exceeded" => Some(
            "ChatGPT plan usage limit reached. Open Manage usage in ChatGPT settings to review your limits.",
        ),
        "subscription_sharing_user_not_eligible" => Some(
            "ChatGPT plan usage is unavailable for this account, workspace, or policy.",
        ),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            Some("ChatGPT plan usage is temporarily unavailable. Try again later.")
        }
        "invalid_grant" | "invalid_refresh_token" | "token_expired" | "refresh_token_expired"
        | "refresh_token_invalidated" | "refresh_token_reused" => {
            Some("The ChatGPT session has expired or been revoked. Reconnect the account.")
        }
        "subscription_sharing_invalid_user" => Some(
            "Check the selected ChatGPT account and permissions; reconnect if access has been revoked.",
        ),
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => Some(
            "ChatGPT plan permissions do not authorize this request. Check the app's grant configuration.",
        ),
        "subscription_sharing_unsupported_capability" => Some(
            "The request contains an unsupported feature. Check the parameter and model.",
        ),
        "subscription_sharing_route_not_supported" => {
            Some("This request route is not supported. Check the integration configuration.")
        }
        "invalid_client" => Some("The ChatGPT app configuration was rejected. Check the client configuration."),
        _ => None,
    }
}

fn failure(
    context: &str,
    status: Option<StatusCode>,
    request_id: Option<&str>,
    body: &Value,
    access_token: &str,
) -> String {
    let mut message = format!("ChatGPT {context}");
    if let Some(status) = status {
        message.push_str(&format!(" (HTTP {})", status.as_u16()));
    }
    let error = body
        .pointer("/response/error")
        .or_else(|| body.get("error"))
        .filter(|error| error.is_object())
        .unwrap_or(body);
    let known_detail_code = [
        "/detail/error/code",
        "/detail/code",
        "/detail/error",
        "/detail",
    ]
    .iter()
    .filter_map(|path| body.pointer(path).and_then(Value::as_str))
    .find(|code| recovery(code).is_some());
    let code = safe_field(error.get("code").and_then(Value::as_str), access_token)
        .or_else(|| safe_field(body.get("code").and_then(Value::as_str), access_token))
        .or_else(|| {
            safe_field(body.get("error").and_then(Value::as_str), access_token)
                .filter(|code| recovery(code).is_some())
        })
        .or_else(|| safe_field(known_detail_code, access_token));
    if let Some(code) = code {
        message.push_str(&format!("; code: {code}"));
    }
    let parameter = error
        .get("param")
        .and_then(Value::as_str)
        .or_else(|| body.pointer("/detail/error/param").and_then(Value::as_str))
        .or_else(|| body.pointer("/detail/param").and_then(Value::as_str));
    if let Some(parameter) = safe_field(parameter, access_token) {
        message.push_str(&format!("; parameter: {parameter}"));
    }
    if let Some(reason) = safe_field(
        body.pointer("/response/incomplete_details/reason")
            .and_then(Value::as_str),
        access_token,
    ) {
        message.push_str(&format!("; reason: {reason}"));
    }
    if let Some(request_id) = safe_field(request_id, access_token) {
        message.push_str(&format!("; request ID: {request_id}"));
    }
    let guidance = code.and_then(recovery).or_else(|| match status.map(|s| s.as_u16()) {
        Some(401) => Some("Check the selected account and its permissions."),
        Some(403) => Some("Check the account, workspace, and integration restrictions."),
        Some(429) => Some("ChatGPT is limiting requests. Try again later or open Manage usage to review your limits."),
        Some(503) => Some("The service may be temporarily unavailable. Try again later."),
        _ => None,
    });
    if let Some(guidance) = guidance {
        message.push_str(". ");
        message.push_str(guidance);
    }
    message
}

async fn checked_response(
    response: Response,
    cancel: &CancellationToken,
    access_token: &str,
) -> Result<(Response, Option<String>), String> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| safe_field(Some(value), access_token))
        .map(str::to_owned);
    if status.is_success() {
        return Ok((response, request_id));
    }
    let body = or_cancel(cancel, response.bytes()).await?.map_err(|_| {
        failure(
            "request failed",
            Some(status),
            request_id.as_deref(),
            &Value::Null,
            access_token,
        )
    })?;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Err(failure(
        "request failed",
        Some(status),
        request_id.as_deref(),
        &value,
        access_token,
    ))
}

pub(super) async fn models(
    client: &Client,
    access_token: &str,
    cancel: &CancellationToken,
) -> Result<Vec<PlanModel>, String> {
    models_at(client, access_token, cancel, MODELS_URL).await
}

async fn models_at(
    client: &Client,
    access_token: &str,
    cancel: &CancellationToken,
    url: &str,
) -> Result<Vec<PlanModel>, String> {
    let response = or_cancel(cancel, client.get(url).bearer_auth(access_token).send())
        .await?
        .map_err(|_| {
            "Could not connect to ChatGPT. Check your connection and try again.".to_string()
        })?;
    let (response, _) = checked_response(response, cancel, access_token).await?;
    let body = or_cancel(cancel, response.bytes())
        .await?
        .map_err(|_| "Could not read the ChatGPT model list.".to_string())?;
    let body: Value = serde_json::from_slice(&body)
        .map_err(|_| "ChatGPT returned an invalid model list.".to_string())?;
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or("ChatGPT returned an invalid model list.")?;
    entries
        .iter()
        .filter(|entry| entry.get("visibility").and_then(Value::as_str) == Some("list"))
        .map(|entry| {
            let slug = entry
                .get("slug")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let name = entry
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            match (slug, name) {
                (Some(slug), Some(display_name)) => Ok(PlanModel {
                    slug: slug.to_string(),
                    display_name: display_name.to_string(),
                }),
                _ => Err("ChatGPT returned an invalid model list.".to_string()),
            }
        })
        .collect()
}

pub(crate) async fn complete(
    client: &Client,
    access_token: &str,
    model: &str,
    instructions: &str,
    input: &str,
    cancel: &CancellationToken,
) -> Result<String, String> {
    complete_at(
        client,
        access_token,
        model,
        instructions,
        input,
        cancel,
        RESPONSES_URL,
    )
    .await
}

async fn complete_at(
    client: &Client,
    access_token: &str,
    model: &str,
    instructions: &str,
    input: &str,
    cancel: &CancellationToken,
    url: &str,
) -> Result<String, String> {
    let request = client.post(url).bearer_auth(access_token).json(&json!({
        "model": model,
        "instructions": instructions,
        "input": [{ "role": "user", "content": input }],
        "store": false,
        "stream": true
    }));
    let response = or_cancel(cancel, request.send()).await?.map_err(|_| {
        "Could not connect to ChatGPT. Check your connection and try again.".to_string()
    })?;
    let (response, request_id) = checked_response(response, cancel, access_token).await?;
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    let mut text = String::new();
    loop {
        let chunk = or_cancel(cancel, stream.next()).await?;
        let (data, eof) = match chunk {
            Some(Ok(data)) => (data.to_vec(), false),
            Some(Err(_)) => return Err("ChatGPT response stream was interrupted.".to_string()),
            None => (Vec::new(), true),
        };
        for event in parser.push(&data, eof)? {
            if event.data == "[DONE]" {
                return Err("ChatGPT response ended without completion.".to_string());
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| "ChatGPT returned an invalid response event.".to_string())?;
            let payload_type = value.get("type").and_then(Value::as_str);
            if let (Some(payload_type), false) = (payload_type, event.name.is_empty()) {
                if payload_type != event.name {
                    return Err("ChatGPT returned an inconsistent response event.".to_string());
                }
            }
            let kind = payload_type.unwrap_or(&event.name);
            match kind {
                "response.output_text.delta" => {
                    let delta = value
                        .get("delta")
                        .and_then(Value::as_str)
                        .ok_or("ChatGPT returned an invalid text event.")?;
                    text.push_str(delta);
                }
                "response.failed" | "response.incomplete" | "error" => {
                    return Err(failure(
                        "response failed",
                        None,
                        request_id.as_deref(),
                        &value,
                        access_token,
                    ));
                }
                "response.completed" => {
                    if value
                        .pointer("/response/status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| status != "completed")
                        || value
                            .pointer("/response/error")
                            .is_some_and(|error| !error.is_null())
                    {
                        return Err(failure(
                            "response failed",
                            None,
                            request_id.as_deref(),
                            &value,
                            access_token,
                        ));
                    }
                    if let Some(output) = completed_text(&value) {
                        text = output;
                    }
                    if cancel.is_cancelled() {
                        return Err(CANCELLED.to_string());
                    }
                    if text.trim().is_empty() {
                        return Err("ChatGPT returned no visible text.".to_string());
                    }
                    return Ok(text);
                }
                _ => {}
            }
        }
        if eof {
            return Err("ChatGPT response ended without completion.".to_string());
        }
    }
}

fn completed_text(value: &Value) -> Option<String> {
    let output = value.pointer("/response/output")?.as_array()?;
    let mut text = String::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message")
            || item.get("role").and_then(Value::as_str) != Some("assistant")
        {
            continue;
        }
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                    if let Some(part) = part.get("text").and_then(Value::as_str) {
                        text.push_str(part);
                    }
                }
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
    event_name: String,
    data: String,
    first_line_read: bool,
}

struct SseEvent {
    name: String,
    data: String,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8], eof: bool) -> Result<Vec<SseEvent>, String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(relative_end) = self.buffer[start..]
            .iter()
            .position(|b| *b == b'\r' || *b == b'\n')
        {
            let end = start + relative_end;
            if self.buffer[end] == b'\r' && end + 1 == self.buffer.len() && !eof {
                break;
            }
            let mut line = std::str::from_utf8(&self.buffer[start..end])
                .map_err(|_| "ChatGPT returned an invalid UTF-8 response stream.".to_string())?;
            if !self.first_line_read {
                line = line.strip_prefix('\u{feff}').unwrap_or(line);
                self.first_line_read = true;
            }
            if line.is_empty() {
                if !self.data.is_empty() {
                    self.data.pop();
                    events.push(SseEvent {
                        name: std::mem::take(&mut self.event_name),
                        data: std::mem::take(&mut self.data),
                    });
                } else {
                    self.event_name.clear();
                }
            } else if !line.starts_with(':') {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "data" => {
                        self.data.push_str(value);
                        self.data.push('\n');
                    }
                    "event" => self.event_name = value.to_string(),
                    _ => {}
                }
            }
            start = end + 1;
            if self.buffer[end] == b'\r' && self.buffer.get(start) == Some(&b'\n') {
                start += 1;
            }
        }
        self.buffer.drain(..start);
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;

    async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    return request;
                }
            }
        }
    }

    async fn mock_response(
        status: &str,
        extra_headers: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}", body.len());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (url, task)
    }

    fn client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }

    async fn response_result(body: &str) -> Result<String, String> {
        let (url, task) =
            mock_response("200 OK", "Content-Type: text/event-stream\r\n", body).await;
        let result = complete_at(
            &client(),
            "test-token",
            "model",
            "instructions",
            "input",
            &CancellationToken::new(),
            &url,
        )
        .await;
        task.await.unwrap();
        result
    }

    #[tokio::test]
    async fn model_catalog_is_account_specific_and_preserves_visible_order() {
        let body = r#"{"models":[{"slug":"b","display_name":"Bee","visibility":"list"},{"visibility":"hidden"},{"slug":"a","display_name":"Aye","visibility":"list"}]}"#;
        let (url, task) = mock_response("200 OK", "", body).await;
        let models = models_at(
            &client(),
            "test-token",
            &CancellationToken::new(),
            &format!("{url}/v1/models"),
        )
        .await
        .unwrap();
        let request = String::from_utf8(task.await.unwrap()).unwrap();
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token"));
        assert_eq!(
            serde_json::to_value(models).unwrap(),
            json!([
                {"slug":"b","displayName":"Bee"}, {"slug":"a","displayName":"Aye"}
            ])
        );
        let (url, task) = mock_response(
            "200 OK",
            "",
            r#"{"models":[{"visibility":"list","slug":"bad"}]}"#,
        )
        .await;
        assert!(
            models_at(&client(), "token", &CancellationToken::new(), &url)
                .await
                .is_err()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn responses_request_uses_supported_options_and_only_returns_visible_output() {
        let body = concat!(
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"private\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[",
            "{\"type\":\"reasoning\",\"content\":[{\"type\":\"output_text\",\"text\":\"private\"}]},",
            "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello 🌍\"}]}]}}\n\n"
        );
        let (url, task) =
            mock_response("200 OK", "Content-Type: text/event-stream\r\n", body).await;
        let result = complete_at(
            &client(),
            "test-token",
            "selected",
            "system",
            "meeting",
            &CancellationToken::new(),
            &format!("{url}/v1/responses"),
        )
        .await
        .unwrap();
        assert_eq!(result, "Hello 🌍");
        let request = String::from_utf8(task.await.unwrap()).unwrap();
        assert!(request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body,
            json!({"model":"selected","instructions":"system","input":[{"role":"user","content":"meeting"}],"store":false,"stream":true})
        );
    }

    #[test]
    fn event_parser_handles_split_utf8_crlf_multiline_and_eof() {
        let wire = "\u{feff}: comment\r\nevent: response.output_text.delta\r\ndata: {\"type\":\r\ndata: \"response.output_text.delta\",\"delta\":\"🌍\"}\r\n\r\n";
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        for byte in wire.as_bytes() {
            events.extend(parser.push(&[*byte], false).unwrap());
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "response.output_text.delta");
        assert_eq!(
            serde_json::from_str::<Value>(&events[0].data).unwrap()["delta"],
            "🌍"
        );
        assert!(parser
            .push(b"data: {\"type\":\"response.completed\"}", true)
            .unwrap()
            .is_empty());
        let mut parser = SseParser::default();
        assert_eq!(parser.push(b"data: x\r\r", true).unwrap().len(), 1);
        assert!(SseParser::default()
            .push(&[b'd', b'a', b't', b'a', b':', b' ', 0xff, b'\n'], false)
            .is_err());
    }

    #[tokio::test]
    async fn incomplete_failed_truncated_or_empty_streams_never_return_partial_text() {
        let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n";
        let failure_event = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n\n";
        let err = response_result(&format!("{delta}{failure_event}"))
            .await
            .unwrap_err();
        assert!(err.contains("subscription_sharing_usage_limit_exceeded"));
        assert!(err.contains("Manage usage"));
        assert!(!err.contains("partial"));
        for ending in [
            "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            "data: {\"type\":\"error\",\"code\":\"subscription_sharing_usage_unavailable\"}\n\n",
            "data: {\"type\":\"response.completed\"}",
            "data: [DONE]\n\n",
            "",
        ] {
            assert!(response_result(&format!("{delta}{ending}")).await.is_err(), "ending: {ending}");
        }
        assert!(
            response_result("data: {\"type\":\"response.completed\"}\n\n")
                .await
                .unwrap_err()
                .contains("no visible text")
        );
    }

    #[tokio::test]
    async fn admission_and_usage_failures_keep_safe_identifiers() {
        for (status, body, expected) in [
            (
                "403 Forbidden",
                r#"{"detail":"test-token private input"}"#,
                "integration restrictions",
            ),
            (
                "429 Too Many Requests",
                r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","param":"model","message":"test-token private input"}}"#,
                "subscription_sharing_usage_limit_exceeded",
            ),
        ] {
            let (url, task) = mock_response(status, "x-request-id: req-123\r\n", body).await;
            let err = complete_at(
                &client(),
                "test-token",
                "model",
                "instructions",
                "input",
                &CancellationToken::new(),
                &url,
            )
            .await
            .unwrap_err();
            task.await.unwrap();
            assert!(err.contains(expected));
            assert!(err.contains("req-123"));
            assert!(!err.contains("test-token"));
            assert!(!err.contains("private input"));
        }
    }

    #[tokio::test]
    async fn terminal_stream_failures_include_recovery_after_partial_text() {
        for (code, guidance) in [
            ("subscription_sharing_usage_limit_exceeded", "Manage usage"),
            (
                "subscription_sharing_user_not_eligible",
                "unavailable for this account",
            ),
            ("subscription_sharing_usage_unavailable", "Try again later"),
            ("subscription_sharing_user_unavailable", "Try again later"),
            ("invalid_refresh_token", "Reconnect the account"),
            (
                "subscription_sharing_invalid_user",
                "reconnect if access has been revoked",
            ),
            (
                "subscription_sharing_unsupported_capability",
                "unsupported feature",
            ),
            (
                "subscription_sharing_route_not_supported",
                "route is not supported",
            ),
            ("chatpass_v2_scope_not_authorized", "grant configuration"),
        ] {
            let event = json!({"type":"response.failed", "response":{"error":{
                "code":code, "message":"test-token private input"
            }}});
            let body = format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}}\n\ndata: {event}\n\n");
            let err = response_result(&body).await.unwrap_err();
            assert!(err.contains(code), "{err}");
            assert!(err.contains(guidance), "{err}");
            assert!(
                !err.contains("test-token")
                    && !err.contains("private input")
                    && !err.contains("partial")
            );
            assert!(!err.contains("response body contained"));
        }
    }

    #[tokio::test]
    async fn admission_detail_and_oauth_errors_use_known_safe_codes() {
        for (status, body, code, guidance) in [
            (
                "429 Too Many Requests",
                r#"{"detail":"subscription_sharing_usage_limit_exceeded"}"#,
                "subscription_sharing_usage_limit_exceeded",
                "Manage usage",
            ),
            (
                "403 Forbidden",
                r#"{"detail":{"error":{"code":"subscription_sharing_user_not_eligible","message":"test-token private input"}}}"#,
                "subscription_sharing_user_not_eligible",
                "unavailable for this account",
            ),
            (
                "503 Service Unavailable",
                r#"{"detail":{"code":"subscription_sharing_usage_unavailable"}}"#,
                "subscription_sharing_usage_unavailable",
                "Try again later",
            ),
            (
                "401 Unauthorized",
                r#"{"error":"invalid_grant","error_description":"test-token private input"}"#,
                "invalid_grant",
                "Reconnect the account",
            ),
            (
                "429 Too Many Requests",
                r#"{"detail":"test-token private input"}"#,
                "HTTP 429",
                "Manage usage",
            ),
        ] {
            let (url, task) = mock_response(status, "x-request-id: req-123\r\n", body).await;
            let err = complete_at(
                &client(),
                "test-token",
                "model",
                "instructions",
                "input",
                &CancellationToken::new(),
                &url,
            )
            .await
            .unwrap_err();
            task.await.unwrap();
            assert!(err.contains(code), "{err}");
            assert!(err.contains(guidance), "{err}");
            assert!(err.contains("req-123"));
            assert!(!err.contains("test-token") && !err.contains("private input"));
            assert!(!err.contains("response body contained"));
        }
    }

    #[test]
    fn error_identifiers_do_not_echo_the_access_token() {
        let body = json!({"error":{"code":"test-token", "param":"test-token", "message":"test-token private input"}});
        let err = failure(
            "request failed",
            Some(StatusCode::FORBIDDEN),
            Some("test-token"),
            &body,
            "test-token",
        );
        assert!(!err.contains("test-token") && !err.contains("private input"));
        assert!(err.contains("HTTP 403"));
    }

    #[tokio::test]
    async fn cancellation_interrupts_request_and_stream_waits() {
        for send_headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (ready_tx, ready_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let _ = read_request(&mut stream).await;
                if send_headers {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 100\r\n\r\n").await.unwrap();
                }
                ready_tx.send(()).unwrap();
                let _ = release_rx.await;
            });
            let cancel = CancellationToken::new();
            let client = client();
            let future = complete_at(&client, "token", "model", "system", "input", &cancel, &url);
            tokio::pin!(future);
            tokio::select! {
                result = &mut future => panic!("request ended early: {result:?}"),
                _ = ready_rx => {}
            }
            cancel.cancel();
            let result = tokio::time::timeout(Duration::from_secs(1), &mut future)
                .await
                .unwrap();
            assert_eq!(result.unwrap_err(), CANCELLED);
            let _ = release_tx.send(());
            task.await.unwrap();
        }
    }
}
