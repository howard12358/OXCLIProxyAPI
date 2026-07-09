pub mod health {
    use serde::{Deserialize, Serialize};

    /// 数据平面健康状态枚举，供 `/healthz`、`/readyz` 和 runtime 元数据复用。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "snake_case")]
    pub enum ServiceState {
        #[default]
        Starting,
        Ready,
        Degraded,
        Failed,
    }
}

pub mod routing {
    use serde::{Deserialize, Serialize};

    use crate::upstream::ProviderKind;

    /// 记录一次选路结果来自哪种粘性/策略语义。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StickinessSource {
        Strategy,
        SessionAffinity,
        ReboundSessionAffinity,
        PinnedAuth,
    }

    /// 请求进入真实执行前的路由决策结果。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ExecutionPlan {
        pub provider: ProviderKind,
        pub model: String,
        pub auth_id: String,
        #[serde(default)]
        pub retry_candidates: Vec<String>,
        pub stickiness_source: StickinessSource,
    }
}

pub mod snapshot {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    /// Go 导出给 Rust 数据平面的 runtime snapshot 总契约。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct RuntimeSnapshot {
        pub version: String,
        pub generated_at: String,
        pub source_instance_id: String,
        #[serde(default)]
        pub listeners: ListenerConfig,
        #[serde(default)]
        pub routes: RouteConfig,
        #[serde(default)]
        pub routing: RoutingConfig,
        #[serde(default)]
        pub providers: BTreeMap<String, ProviderConfig>,
        #[serde(default)]
        pub model_aliases: BTreeMap<String, BTreeMap<String, String>>,
        #[serde(default)]
        pub models: BTreeMap<String, Vec<String>>,
        #[serde(default)]
        pub auth_pool: Vec<AuthRecord>,
        #[serde(default)]
        pub network: NetworkConfig,
        #[serde(default)]
        pub usage_queue: UsageQueueConfig,
        #[serde(default)]
        pub feature_flags: BTreeMap<String, bool>,
    }

    /// 监听器配置，描述 Go 管理面公开出来的地址信息。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct ListenerConfig {
        #[serde(default)]
        pub public_http: String,
    }

    /// 路由开关配置，决定哪些入口可由 Rust 数据平面承接。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct RouteConfig {
        #[serde(default)]
        pub responses: bool,
        #[serde(default)]
        pub chat_completions: bool,
        #[serde(default)]
        pub messages: bool,
    }

    /// 路由策略配置，包括调度策略和会话粘性 TTL。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RoutingConfig {
        pub strategy: RoutingStrategy,
        #[serde(default)]
        pub session_affinity: bool,
        #[serde(default = "default_session_ttl_seconds")]
        pub session_ttl_seconds: u64,
    }

    impl Default for RoutingConfig {
        fn default() -> Self {
            Self {
                strategy: RoutingStrategy::FillFirst,
                session_affinity: true,
                session_ttl_seconds: default_session_ttl_seconds(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "kebab-case")]
    pub enum RoutingStrategy {
        #[default]
        FillFirst,
        RoundRobin,
    }

    /// provider 级启停配置。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct ProviderConfig {
        #[serde(default)]
        pub enabled: bool,
    }

    /// 单个 auth 的运行时快照记录。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct AuthRecord {
        pub id: String,
        #[serde(default)]
        pub auth_index: String,
        pub provider: String,
        #[serde(default)]
        pub auth_kind: String,
        #[serde(default)]
        pub usage_source: String,
        #[serde(default)]
        pub priority: i32,
        #[serde(default = "default_true")]
        pub enabled: bool,
        #[serde(default)]
        pub supports_models: Vec<String>,
        #[serde(default)]
        pub labels: Vec<String>,
        #[serde(default)]
        pub execution: AuthExecution,
        pub cooldown_until: Option<String>,
    }

    /// provider-specific 执行信息容器。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct AuthExecution {
        pub codex: Option<CodexExecution>,
    }

    /// Codex OAuth / 执行侧所需的最小运行时字段。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct CodexExecution {
        pub access_token: String,
        #[serde(default)]
        pub account_id: String,
        #[serde(default)]
        pub base_url: String,
        #[serde(default)]
        pub user_agent: String,
        #[serde(default)]
        pub openai_beta: String,
    }

    /// CPA usage queue 的最小启用配置。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct UsageQueueConfig {
        #[serde(default)]
        pub enabled: bool,
        #[serde(default)]
        pub backend: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub external: Option<ExternalUsageQueueConfig>,
    }

    /// 外部 usage queue 推送配置，用于 Home 模式直接 LPUSH 到 Redis/CPA queue。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ExternalUsageQueueConfig {
        #[serde(default = "default_external_usage_address")]
        pub address: String,
        #[serde(default)]
        pub password: String,
        #[serde(default = "default_external_usage_key")]
        pub key: String,
        #[serde(default = "default_external_usage_timeout_ms")]
        pub timeout_ms: u64,
    }

    fn default_external_usage_address() -> String {
        String::new()
    }

    fn default_external_usage_key() -> String {
        "usage".to_string()
    }

    fn default_external_usage_timeout_ms() -> u64 {
        5000
    }

    /// 网络层附加配置，目前主要承载统一上游代理。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct NetworkConfig {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub upstream_proxy: Option<String>,
    }

    const fn default_session_ttl_seconds() -> u64 {
        3600
    }

    const fn default_true() -> bool {
        true
    }
}

pub mod upstream {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    /// Rust 数据平面内部识别的上游 provider 类型。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ProviderKind {
        OpenAi,
        Codex,
        Mock,
    }

    /// 上游 HTTP 响应头的统一抽象。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct UpstreamResponseHead {
        pub status: u16,
        #[serde(default)]
        pub headers: BTreeMap<String, String>,
    }

    /// 上游执行链路内部使用的通用流事件抽象。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum StreamEvent {
        Headers(UpstreamResponseHead),
        Data { bytes: Vec<u8> },
        Terminal { status: &'static str },
        Error { message: String },
    }
}
