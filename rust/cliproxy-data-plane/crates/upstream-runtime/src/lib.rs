use std::{
    collections::HashMap,
    collections::BTreeMap,
    env,
    pin::Pin,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use cliproxy_common_types::{
    snapshot::{AuthRecord, CodexExecution},
    upstream::{ProviderKind, StreamEvent, UpstreamResponseHead},
};
use futures_util::{Stream, StreamExt};
use reqwest::{
    Client, Method, Response,
    header::{ACCEPT, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};
use tracing::info;
use url::Url;
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;
const CODEX_ORIGINATOR: &str = "codex-tui";

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub base_url: String,
    pub token: String,
    pub user_agent: String,
    pub openai_beta: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpstreamRuntimeConfig {
    pub upstream_proxy: Option<String>,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub openai: Option<OpenAiConfig>,
    pub codex: Option<CodexConfig>,
}

#[derive(Clone)]
pub struct UpstreamRuntime {
    config: Arc<UpstreamRuntimeConfig>,
    default_client: Client,
    proxy_clients: Arc<RwLock<HashMap<String, Client>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxySetting {
    Inherit,
    Direct,
    Proxy(Url),
}

impl ProxySetting {
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Self::Inherit);
        }
        if trimmed.eq_ignore_ascii_case("direct") || trimmed.eq_ignore_ascii_case("none") {
            return Ok(Self::Direct);
        }

        let parsed = Url::parse(trimmed).with_context(|| format!("invalid proxy URL: {trimmed}"))?;
        match parsed.scheme() {
            "http" | "https" | "socks5" | "socks5h" => Ok(Self::Proxy(parsed)),
            other => bail!("unsupported proxy scheme: {other}"),
        }
    }

    fn cache_key(&self) -> String {
        match self {
            Self::Inherit => "inherit".to_string(),
            Self::Direct => "direct".to_string(),
            Self::Proxy(url) => format!("proxy:{}", url),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamRequest {
    pub model: String,
    pub body: Vec<u8>,
    pub stream: bool,
}

#[derive(Debug)]
pub struct UpstreamResponse {
    pub provider: ProviderKind,
    pub body: Bytes,
    pub events: Vec<StreamEvent>,
    pub head: UpstreamResponseHead,
}

pub struct UpstreamStreamResponse {
    pub provider: ProviderKind,
    pub first_chunk: Bytes,
    pub stream: ByteStream,
    pub events: Vec<StreamEvent>,
    pub head: UpstreamResponseHead,
}

impl UpstreamRuntime {
    pub fn new(config: UpstreamRuntimeConfig) -> Self {
        let default_proxy = preferred_proxy_setting(&config, None)
            .expect("failed to resolve upstream proxy setting");
        let client = build_client(&config, &default_proxy).expect("failed to build upstream http client");
        Self {
            config: Arc::new(config),
            default_client: client,
            proxy_clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.openai.is_some() || self.config.codex.is_some()
    }

    pub fn provider_for_model(&self, model: &str) -> Option<ProviderKind> {
        let model = model.trim().to_ascii_lowercase();
        if model.contains("codex") && self.config.codex.is_some() {
            return Some(ProviderKind::Codex);
        }
        if self.config.openai.is_some() {
            return Some(ProviderKind::OpenAi);
        }
        if self.config.codex.is_some() {
            return Some(ProviderKind::Codex);
        }
        None
    }

    pub fn can_execute_for_auth(&self, auth: &AuthRecord) -> bool {
        match auth.provider.trim().to_ascii_lowercase().as_str() {
            "codex" => auth
                .execution
                .codex
                .as_ref()
                .map(|execution| {
                    non_empty(Some(execution.access_token.as_str())).is_some()
                        && (non_empty(Some(execution.base_url.as_str())).is_some()
                            || self.config.codex.is_some())
                })
                .unwrap_or(false),
            _ => false,
        }
    }

    pub async fn execute_responses(
        &self,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        let provider = self.provider_for_model(&request.model).ok_or_else(|| {
            anyhow!(
                "no upstream provider configured for model {}",
                request.model
            )
        })?;

        match provider {
            ProviderKind::OpenAi => self.execute_openai(request, proxy_override).await,
            ProviderKind::Codex => self.execute_codex(request, proxy_override).await,
            ProviderKind::Mock => bail!("mock provider is not handled by upstream runtime"),
        }
    }

    pub async fn execute_responses_for_auth(
        &self,
        auth: &AuthRecord,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        let provider = match auth.provider.trim().to_ascii_lowercase().as_str() {
            "codex" => ProviderKind::Codex,
            "openai" | "openai-compatibility" => ProviderKind::OpenAi,
            other => bail!("unsupported auth provider for upstream execution: {other}"),
        };

        match provider {
            ProviderKind::Codex => self.execute_codex_with_auth(auth, request, proxy_override).await,
            ProviderKind::OpenAi => self.execute_openai(request, proxy_override).await,
            ProviderKind::Mock => bail!("mock provider is not handled by upstream runtime"),
        }
    }

    async fn execute_openai(
        &self,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        let config = self
            .config
            .openai
            .as_ref()
            .ok_or_else(|| anyhow!("openai upstream is not configured"))?;
        let url = format!("{}/responses", config.base_url.trim_end_matches('/'));
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.api_key))
                .context("invalid openai api key header value")?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        apply_transport_headers(&mut headers, request.stream);
        self.execute_http(ProviderKind::OpenAi, &url, headers, request, proxy_override)
            .await
    }

    async fn execute_codex(
        &self,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        let config = self
            .config
            .codex
            .as_ref()
            .ok_or_else(|| anyhow!("codex upstream is not configured"))?;
        let url = format!("{}/responses", config.base_url.trim_end_matches('/'));
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))
                .context("invalid codex token header value")?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&config.user_agent).context("invalid codex user agent value")?,
        );
        if let Some(beta) = &config.openai_beta {
            headers.insert(
                "OpenAI-Beta",
                HeaderValue::from_str(beta).context("invalid codex openai-beta header value")?,
            );
        }
        apply_transport_headers(&mut headers, request.stream);
        apply_codex_headers(&mut headers, &config.user_agent, None)?;
        self.execute_http(ProviderKind::Codex, &url, headers, request, proxy_override)
            .await
    }

    async fn execute_codex_with_auth(
        &self,
        auth: &AuthRecord,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        let execution = auth
            .execution
            .codex
            .as_ref()
            .ok_or_else(|| anyhow!("codex auth {} missing execution.codex", auth.id))?;
        let config = self.config.codex.as_ref();
        let url = format!(
            "{}/responses",
            codex_base_url(execution, config)?.trim_end_matches('/')
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", execution.access_token))
                .context("invalid codex auth access token header value")?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&codex_user_agent(execution, config))
                .context("invalid codex auth user agent value")?,
        );
        if let Some(account_id) = non_empty(Some(execution.account_id.as_str())) {
            headers.insert(
                "Chatgpt-Account-Id",
                HeaderValue::from_str(account_id)
                    .context("invalid codex account id header value")?,
            );
        }
        if let Some(beta) = codex_openai_beta(execution, config) {
            headers.insert(
                "OpenAI-Beta",
                HeaderValue::from_str(&beta).context("invalid codex openai-beta header value")?,
            );
        }
        apply_transport_headers(&mut headers, request.stream);
        apply_codex_headers(
            &mut headers,
            &codex_user_agent(execution, config),
            non_empty(Some(execution.account_id.as_str())),
        )?;
        self.execute_http(ProviderKind::Codex, &url, headers, request, proxy_override)
            .await
    }

    async fn execute_http(
        &self,
        provider: ProviderKind,
        url: &str,
        headers: HeaderMap,
        request: UpstreamRequest,
        proxy_override: Option<&str>,
    ) -> Result<UpstreamExecutionResult> {
        info!(provider = ?provider, url, stream = request.stream, "dispatching upstream responses request");

        let response = self
            .client_for_proxy(proxy_override)?
            .request(Method::POST, url)
            .headers(headers)
            .body(request.body)
            .send()
            .await
            .with_context(|| format!("failed to call upstream {provider:?} responses endpoint"))?;

        let head = response_head(&response);
        if !response.status().is_success() {
            let error_body = response
                .bytes()
                .await
                .context("failed to read upstream error body")?;
            bail!(
                "upstream {} error {}: {}",
                provider_name(provider),
                head.status,
                String::from_utf8_lossy(&error_body)
            );
        }

        if request.stream {
            let mut stream = response.bytes_stream();
            let first_chunk = stream
                .next()
                .await
                .transpose()
                .context("failed to receive first upstream stream chunk")?
                .ok_or_else(|| anyhow!("upstream stream produced no bootstrap chunk"))?;

            let mut events = vec![StreamEvent::Headers(head.clone())];
            events.push(StreamEvent::Data {
                bytes: first_chunk.to_vec(),
            });

            let mapped = stream.map(|item| item.context("failed to receive upstream stream chunk"));
            Ok(UpstreamExecutionResult::Streaming(UpstreamStreamResponse {
                provider,
                first_chunk,
                stream: Box::pin(mapped),
                events,
                head,
            }))
        } else {
            let body = response
                .bytes()
                .await
                .context("failed to read upstream response body")?;
            let events = vec![
                StreamEvent::Headers(head.clone()),
                StreamEvent::Data {
                    bytes: body.to_vec(),
                },
                StreamEvent::Terminal {
                    status: "completed",
                },
            ];
            Ok(UpstreamExecutionResult::NonStreaming(UpstreamResponse {
                provider,
                body,
                events,
                head,
            }))
        }
    }

    fn client_for_proxy(&self, proxy_override: Option<&str>) -> Result<Client> {
        let setting = preferred_proxy_setting(&self.config, proxy_override)?;
        let key = setting.cache_key();
        if key == "inherit" {
            return Ok(self.default_client.clone());
        }

        if let Some(client) = self
            .proxy_clients
            .read()
            .expect("proxy client cache lock poisoned")
            .get(&key)
            .cloned()
        {
            return Ok(client);
        }

        let client = build_client(&self.config, &setting)?;
        self.proxy_clients
            .write()
            .expect("proxy client cache lock poisoned")
            .insert(key, client.clone());
        Ok(client)
    }
}

fn build_client(config: &UpstreamRuntimeConfig, setting: &ProxySetting) -> Result<Client> {
    let mut builder = Client::builder();

    match setting {
        ProxySetting::Inherit => {
            builder = apply_inherited_proxy(builder)?;
        }
        ProxySetting::Direct => {
            builder = builder.no_proxy();
        }
        ProxySetting::Proxy(url) => {
            builder = builder.no_proxy().proxy(
                reqwest::Proxy::all(url.as_str())
                    .with_context(|| format!("invalid upstream proxy: {url}"))?,
            );
        }
    }

    if matches!(setting, ProxySetting::Inherit)
        && config.upstream_proxy.as_deref().is_none()
        && (config.http_proxy.is_some() || config.https_proxy.is_some())
    {
        builder = builder.no_proxy();
        if let Some(proxy_url) = config.http_proxy.as_deref() {
            builder = builder.proxy(
                reqwest::Proxy::http(proxy_url)
                    .with_context(|| format!("invalid upstream http proxy: {proxy_url}"))?,
            );
        }
        if let Some(proxy_url) = config.https_proxy.as_deref() {
            builder = builder.proxy(
                reqwest::Proxy::https(proxy_url)
                    .with_context(|| format!("invalid upstream https proxy: {proxy_url}"))?,
            );
        }
    }

    builder.build().context("failed to build upstream reqwest client")
}

fn preferred_proxy_setting(
    config: &UpstreamRuntimeConfig,
    proxy_override: Option<&str>,
) -> Result<ProxySetting> {
    if let Some(proxy) = proxy_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ProxySetting::parse(proxy);
    }

    if let Some(proxy) = config
        .upstream_proxy
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ProxySetting::parse(proxy);
    }

    if config.http_proxy.is_some() || config.https_proxy.is_some() {
        return Ok(ProxySetting::Inherit);
    }

    Ok(inherited_proxy_setting())
}

fn inherited_proxy_setting() -> ProxySetting {
    if let Some(proxy) = first_env_value(&["ALL_PROXY", "all_proxy"]) {
        if let Ok(setting) = ProxySetting::parse(&proxy) {
            return setting;
        }
    }
    ProxySetting::Inherit
}

fn apply_inherited_proxy(builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
    let mut builder = builder;
    if let Some(proxy) = first_env_value(&["ALL_PROXY", "all_proxy"]) {
        builder = builder.proxy(
            reqwest::Proxy::all(&proxy)
                .with_context(|| format!("invalid inherited all_proxy: {proxy}"))?,
        );
    }

    if let Some(proxy) = first_env_value(&["HTTP_PROXY", "http_proxy"]) {
        builder = if is_unified_proxy_scheme(&proxy) {
            builder.proxy(
                reqwest::Proxy::all(&proxy)
                    .with_context(|| format!("invalid inherited http_proxy: {proxy}"))?,
            )
        } else {
            builder.proxy(
                reqwest::Proxy::http(&proxy)
                    .with_context(|| format!("invalid inherited http_proxy: {proxy}"))?,
            )
        };
    }

    if let Some(proxy) = first_env_value(&["HTTPS_PROXY", "https_proxy"]) {
        builder = if is_unified_proxy_scheme(&proxy) {
            builder.proxy(
                reqwest::Proxy::all(&proxy)
                    .with_context(|| format!("invalid inherited https_proxy: {proxy}"))?,
            )
        } else {
            builder.proxy(
                reqwest::Proxy::https(&proxy)
                    .with_context(|| format!("invalid inherited https_proxy: {proxy}"))?,
            )
        };
    }

    Ok(builder)
}

fn first_env_value(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_unified_proxy_scheme(raw: &str) -> bool {
    raw.trim_start().starts_with("socks5://") || raw.trim_start().starts_with("socks5h://")
}

pub enum UpstreamExecutionResult {
    NonStreaming(UpstreamResponse),
    Streaming(UpstreamStreamResponse),
}

fn response_head(response: &Response) -> UpstreamResponseHead {
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.to_string(), value.to_string()))
        })
        .collect::<BTreeMap<_, _>>();

    UpstreamResponseHead {
        status: response.status().as_u16(),
        headers,
    }
}

fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Codex => "codex",
        ProviderKind::Mock => "mock",
    }
}

fn codex_base_url(execution: &CodexExecution, fallback: Option<&CodexConfig>) -> Result<String> {
    if let Some(value) = non_empty(Some(execution.base_url.as_str())) {
        return Ok(value.to_string());
    }
    if let Some(config) = fallback.and_then(|config| non_empty(Some(config.base_url.as_str()))) {
        return Ok(config.to_string());
    }
    bail!("codex base_url is not configured")
}

fn codex_user_agent(execution: &CodexExecution, fallback: Option<&CodexConfig>) -> String {
    non_empty(Some(execution.user_agent.as_str()))
        .or_else(|| fallback.and_then(|config| non_empty(Some(config.user_agent.as_str()))))
        .unwrap_or("cliproxy-data-plane/0.1.0")
        .to_string()
}

fn codex_openai_beta(execution: &CodexExecution, fallback: Option<&CodexConfig>) -> Option<String> {
    non_empty(Some(execution.openai_beta.as_str()))
        .map(ToOwned::to_owned)
        .or_else(|| fallback.and_then(|config| config.openai_beta.clone()))
}

fn apply_transport_headers(headers: &mut HeaderMap, stream: bool) {
    headers.insert(
        ACCEPT,
        HeaderValue::from_static(if stream {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    headers.insert(CONNECTION, HeaderValue::from_static("Keep-Alive"));
}

fn apply_codex_headers(
    headers: &mut HeaderMap,
    user_agent: &str,
    account_id: Option<&str>,
) -> Result<()> {
    headers.insert("Originator", HeaderValue::from_static(CODEX_ORIGINATOR));
    if user_agent.contains("Mac OS") {
        headers.insert(
            "Session_id",
            HeaderValue::from_str(&generated_session_id())
                .context("invalid codex session_id header value")?,
        );
    }
    if let Some(account_id) = account_id {
        headers.insert(
            "Chatgpt-Account-Id",
            HeaderValue::from_str(account_id).context("invalid codex account id header value")?,
        );
    }
    Ok(())
}

fn generated_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("rs-session-{nanos}")
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_for_model_prefers_codex_when_model_contains_codex() {
        let runtime = UpstreamRuntime::new(UpstreamRuntimeConfig {
            upstream_proxy: None,
            http_proxy: None,
            https_proxy: None,
            openai: Some(OpenAiConfig {
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "openai-key".to_string(),
            }),
            codex: Some(CodexConfig {
                base_url: "https://chatgpt.com/backend-api/codex".to_string(),
                token: "codex-token".to_string(),
                user_agent: "cliproxy-test".to_string(),
                openai_beta: None,
            }),
        });

        assert_eq!(
            runtime.provider_for_model("gpt-5-codex"),
            Some(ProviderKind::Codex)
        );
        assert_eq!(
            runtime.provider_for_model("gpt-5"),
            Some(ProviderKind::OpenAi)
        );
    }
}
