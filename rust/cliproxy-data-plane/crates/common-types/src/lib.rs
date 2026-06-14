pub mod health {
    use serde::{Deserialize, Serialize};

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

pub mod snapshot {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

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
        pub usage_queue: UsageQueueConfig,
        #[serde(default)]
        pub feature_flags: BTreeMap<String, bool>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct ListenerConfig {
        #[serde(default)]
        pub public_http: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct RouteConfig {
        #[serde(default)]
        pub responses: bool,
        #[serde(default)]
        pub chat_completions: bool,
        #[serde(default)]
        pub messages: bool,
    }

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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct ProviderConfig {
        #[serde(default)]
        pub enabled: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct AuthRecord {
        pub id: String,
        pub provider: String,
        #[serde(default)]
        pub priority: i32,
        #[serde(default = "default_true")]
        pub enabled: bool,
        #[serde(default)]
        pub supports_models: Vec<String>,
        #[serde(default)]
        pub labels: Vec<String>,
        pub cooldown_until: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct UsageQueueConfig {
        #[serde(default)]
        pub enabled: bool,
        #[serde(default)]
        pub backend: String,
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ProviderKind {
        OpenAi,
        Codex,
        Mock,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct UpstreamResponseHead {
        pub status: u16,
        #[serde(default)]
        pub headers: BTreeMap<String, String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum StreamEvent {
        Headers(UpstreamResponseHead),
        Data { bytes: Vec<u8> },
        Terminal { status: &'static str },
        Error { message: String },
    }
}
