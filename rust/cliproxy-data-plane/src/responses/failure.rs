use anyhow::Error;
use time::{Duration, OffsetDateTime};

use crate::error_events::ErrorScope;

/// 下游首字节提交前，auth 绑定错误是否允许切换到下一个候选 auth。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrecommitRetryClassification {
    Retryable(UpstreamFailureKind),
    NonRetryable(UpstreamFailureKind),
}

impl PrecommitRetryClassification {
    pub(super) fn should_retry(self) -> bool {
        matches!(self, Self::Retryable(_))
    }

    #[cfg(test)]
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Retryable(kind) | Self::NonRetryable(kind) => kind.reason(),
        }
    }
}

/// 当前 Rust `/v1/responses` 需要感知的最小上游失败语义。
///
/// 这里故意不把整个 `anyhow::Error` 传播到策略层，而是先收口成稳定的 typed kind，
/// 避免 cooldown / recorder / retry 各自重复解析字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpstreamFailureKind {
    Auth401,
    Auth403,
    NotFound,
    UsageLimitReached,
    ModelNotSupported,
    TransientUpstream,
    Auth429,
    Other,
}

impl UpstreamFailureKind {
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Auth401 => "auth_401",
            Self::Auth403 => "auth_403",
            Self::NotFound => "not_found",
            Self::UsageLimitReached => "usage_limit_reached",
            Self::ModelNotSupported => "model_not_supported",
            Self::TransientUpstream => "transient_upstream",
            Self::Auth429 => "auth_429",
            Self::Other => "other",
        }
    }

    pub(super) fn scope(self) -> ErrorScope {
        match self {
            Self::Auth401 | Self::Auth403 => ErrorScope::Auth,
            Self::NotFound
            | Self::UsageLimitReached
            | Self::ModelNotSupported
            | Self::TransientUpstream
            | Self::Auth429
            | Self::Other => ErrorScope::Model,
        }
    }

    pub(super) fn quota_exceeded(self) -> bool {
        matches!(self, Self::UsageLimitReached)
    }

    pub(super) fn cooldown_until(
        self,
        now: OffsetDateTime,
        retry_after_ms: u64,
    ) -> Option<OffsetDateTime> {
        match self {
            Self::Auth401 | Self::Auth403 => Some(now + Duration::minutes(30)),
            Self::NotFound | Self::ModelNotSupported => Some(now + Duration::hours(12)),
            Self::UsageLimitReached => {
                retry_after_deadline(now, retry_after_ms).or(Some(now + Duration::seconds(1)))
            }
            Self::TransientUpstream => Some(now + Duration::seconds(60)),
            Self::Auth429 | Self::Other => None,
        }
    }

    pub(super) fn precommit_retry(self) -> PrecommitRetryClassification {
        match self {
            Self::Auth401 | Self::Auth403 | Self::UsageLimitReached => {
                PrecommitRetryClassification::Retryable(self)
            }
            Self::Auth429
            | Self::NotFound
            | Self::ModelNotSupported
            | Self::TransientUpstream
            | Self::Other => PrecommitRetryClassification::NonRetryable(self),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ClassifiedUpstreamFailure {
    pub kind: UpstreamFailureKind,
    pub status_code: u16,
    pub error_code: String,
    pub message: String,
    pub retry_after_ms: u64,
}

pub(super) fn classify_upstream_failure(err: &Error) -> ClassifiedUpstreamFailure {
    let message = err.to_string();
    let status_code = classify_status_code(&message);
    let kind = classify_failure_kind(&message, status_code);
    let error_code = classify_error_code(&message, status_code);
    let retry_after_ms = extract_retry_after_ms(&message);
    ClassifiedUpstreamFailure {
        kind,
        status_code,
        error_code,
        message,
        retry_after_ms,
    }
}

pub(super) fn classify_precommit_retry(err: &Error) -> PrecommitRetryClassification {
    classify_upstream_failure(err).kind.precommit_retry()
}

pub(super) fn should_retry_auth_bound_error(err: &Error) -> bool {
    classify_precommit_retry(err).should_retry()
}

fn classify_failure_kind(message: &str, status_code: u16) -> UpstreamFailureKind {
    match status_code {
        401 => UpstreamFailureKind::Auth401,
        402 | 403 => UpstreamFailureKind::Auth403,
        404 => UpstreamFailureKind::NotFound,
        429 if is_codex_quota_exhaustion_error(&message.to_ascii_lowercase()) => {
            UpstreamFailureKind::UsageLimitReached
        }
        400 | 422 if is_model_support_error_message(message) => {
            UpstreamFailureKind::ModelNotSupported
        }
        408 | 500 | 502 | 503 | 504 => UpstreamFailureKind::TransientUpstream,
        429 => UpstreamFailureKind::Auth429,
        _ => UpstreamFailureKind::Other,
    }
}

fn retry_after_deadline(now: OffsetDateTime, retry_after_ms: u64) -> Option<OffsetDateTime> {
    (retry_after_ms > 0).then_some(now + Duration::milliseconds(retry_after_ms as i64))
}

fn classify_status_code(message: &str) -> u16 {
    let lower = message.to_ascii_lowercase();
    for code in [401u16, 402, 403, 404, 408, 422, 429, 500, 502, 503, 504] {
        if lower.contains(&format!("upstream codex error {code}")) {
            return code;
        }
    }
    0
}

fn classify_error_code(message: &str, status_code: u16) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("invalid_api_key")
        || lower.contains("invalid or expired token")
        || lower.contains("refresh_token_reused")
    {
        return "authentication_error".to_string();
    }
    if lower.contains("usage_limit_reached") {
        return "usage_limit_reached".to_string();
    }
    if is_model_support_error_message(message) {
        return "model_not_supported".to_string();
    }
    if status_code == 404 {
        return "not_found".to_string();
    }
    if matches!(status_code, 408 | 500 | 502 | 503 | 504) {
        return "transient_upstream".to_string();
    }
    "upstream_error".to_string()
}

fn is_model_support_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "model_not_supported",
        "requested model is not supported",
        "unsupported model",
        "not available for your plan",
        "not available for your account",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn extract_retry_after_ms(message: &str) -> u64 {
    extract_json_number(message, "resets_in_seconds").saturating_mul(1000)
}

fn extract_json_number(message: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\":");
    let Some(start) = message.find(&needle) else {
        return 0;
    };
    let mut digits = String::new();
    for ch in message[start + needle.len()..].chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse::<u64>().unwrap_or(0)
}

fn is_codex_quota_exhaustion_error(message: &str) -> bool {
    message.contains("upstream codex error 429")
        && (message.contains("usage_limit_reached")
            || message.contains("the usage limit has been reached"))
}
