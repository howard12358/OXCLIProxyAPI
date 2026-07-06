use anyhow::Context;
use cliproxy_common_types::routing::ExecutionPlan;
use cliproxy_common_types::upstream::ProviderKind;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{DEFAULT_CODEX_INSTRUCTIONS, ResponsesRequest, sse::sse_data_payload};

/// 当前 `/v1/responses` 链路使用的最小规范化请求形态。
///
/// 这一层把下游请求解析与 provider 侧发射规则隔离开。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResponsesRequestIr {
    model: String,
    stream: bool,
    input: Option<Value>,
    instructions: Option<String>,
    metadata: Option<Value>,
    store: Option<bool>,
    extra: BTreeMap<String, Value>,
}

impl ResponsesRequestIr {
    /// 在进入 provider 定制改写前，先捕获下游请求的原始语义。
    pub(super) fn from_downstream_request(request: &ResponsesRequest) -> Self {
        Self {
            model: request.model.clone(),
            stream: request.stream,
            input: request.input.clone(),
            instructions: request.instructions.clone(),
            metadata: request.metadata.clone(),
            store: request.store,
            extra: request.extra.clone(),
        }
    }

    /// 按选中的 provider 发射上游请求形态。
    ///
    /// 当前只有 Codex 需要显式改写，其他 provider 先保持下游请求形态不变。
    pub(super) fn emit_upstream_request(&self, execution_plan: &ExecutionPlan) -> ResponsesRequest {
        let mut request = ResponsesRequest {
            model: execution_plan.model.clone(),
            stream: self.stream,
            input: self.input.clone(),
            instructions: self.instructions.clone(),
            metadata: self.metadata.clone(),
            store: self.store,
            extra: self.extra.clone(),
        };

        if execution_plan.provider != ProviderKind::Codex {
            return request;
        }

        request.store = Some(false);
        request.metadata = None;

        if request
            .instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            request.instructions = Some(DEFAULT_CODEX_INSTRUCTIONS.to_string());
        }

        // 兼容性约束：Codex CLI 新版本会直接发送 Responses 原生数组 input。
        // 只有旧式纯文本 input 需要 lift 成 message array，其他形态必须原样保留。
        match request.input.take() {
            Some(Value::String(text)) => {
                request.input = Some(json!([
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "input_text",
                                "text": text
                            }
                        ]
                    }
                ]));
            }
            other => {
                request.input = other;
            }
        }

        request
    }

    pub(super) fn model(&self) -> &str {
        &self.model
    }
}

/// SSE 修复路径使用的最小类型化流事件边界。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ResponsesStreamEventIr {
    OutputItemDone(OutputItemDoneIr),
    Completed(CompletedResponseIr),
    OtherJson(OtherJsonEventIr),
    Done,
    NonJson(NonJsonEventIr),
}

/// `response.output_item.done` 事件在 IR 层的最小承载结构。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OutputItemDoneIr {
    pub(super) payload: Value,
    pub(super) output_index: Option<usize>,
    pub(super) item: Value,
}

/// `response.completed` 事件在 IR 层的最小承载结构。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CompletedResponseIr {
    pub(super) payload: Value,
    pub(super) response: Value,
}

/// 暂未特殊处理、但已成功解析成 JSON 的其他 SSE 事件。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OtherJsonEventIr {
    pub(super) event_type: String,
    pub(super) payload: Value,
}

/// `data:` 存在但不是合法 JSON 的 SSE 事件。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NonJsonEventIr {
    pub(super) payload: Vec<u8>,
}

impl ResponsesStreamEventIr {
    /// 把标准化后的 SSE frame 解析成当前修复与聚合逻辑所需的最小事件集合。
    pub(super) fn from_sse_frame(frame: &[u8]) -> anyhow::Result<Option<Self>> {
        let Some(payload) = sse_data_payload(frame) else {
            return Ok(None);
        };
        if payload == b"[DONE]" {
            return Ok(Some(Self::Done));
        }

        let value = match serde_json::from_slice::<Value>(&payload) {
            Ok(value) => value,
            Err(_) => return Ok(Some(Self::NonJson(NonJsonEventIr { payload }))),
        };

        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let event = match event_type.as_str() {
            "response.output_item.done" => {
                let item = value
                    .get("item")
                    .cloned()
                    .context("response.output_item.done missing item")?;
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize);
                Self::OutputItemDone(OutputItemDoneIr {
                    payload: value,
                    output_index,
                    item,
                })
            }
            "response.completed" => {
                let response = value
                    .get("response")
                    .cloned()
                    .context("response.completed missing response")?;
                Self::Completed(CompletedResponseIr {
                    payload: value,
                    response,
                })
            }
            _ => Self::OtherJson(OtherJsonEventIr {
                event_type,
                payload: value,
            }),
        };

        Ok(Some(event))
    }
}
