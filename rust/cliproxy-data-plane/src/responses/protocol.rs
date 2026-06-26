use anyhow::Context;
use cliproxy_common_types::routing::ExecutionPlan;
use cliproxy_common_types::upstream::ProviderKind;
use serde_json::{Value, json};

use super::{DEFAULT_CODEX_INSTRUCTIONS, ResponsesRequest, sse::sse_data_payload};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResponsesRequestIr {
    model: String,
    stream: bool,
    input: Option<Value>,
    instructions: Option<String>,
    metadata: Option<Value>,
    store: Option<bool>,
}

impl ResponsesRequestIr {
    pub(super) fn from_downstream_request(request: &ResponsesRequest) -> Self {
        Self {
            model: request.model.clone(),
            stream: request.stream,
            input: request.input.clone(),
            instructions: request.instructions.clone(),
            metadata: request.metadata.clone(),
            store: request.store,
        }
    }

    pub(super) fn emit_upstream_request(&self, execution_plan: &ExecutionPlan) -> ResponsesRequest {
        let mut request = ResponsesRequest {
            model: execution_plan.model.clone(),
            stream: self.stream,
            input: self.input.clone(),
            instructions: self.instructions.clone(),
            metadata: self.metadata.clone(),
            store: self.store,
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

        if let Some(Value::String(text)) = request.input.take() {
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

        request
    }

    pub(super) fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ResponsesStreamEventIr {
    OutputItemDone(OutputItemDoneIr),
    Completed(CompletedResponseIr),
    OtherJson(OtherJsonEventIr),
    Done,
    NonJson(NonJsonEventIr),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OutputItemDoneIr {
    pub(super) payload: Value,
    pub(super) output_index: Option<usize>,
    pub(super) item: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CompletedResponseIr {
    pub(super) payload: Value,
    pub(super) response: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OtherJsonEventIr {
    pub(super) event_type: String,
    pub(super) payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct NonJsonEventIr {
    pub(super) payload: Vec<u8>,
}

impl ResponsesStreamEventIr {
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
