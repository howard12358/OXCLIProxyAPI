use bytes::Bytes;
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct ResponsesSseFramer {
    pending: Vec<u8>,
    output_items: std::collections::BTreeMap<usize, Vec<u8>>,
    unindexed_output_items: Vec<Vec<u8>>,
}

impl ResponsesSseFramer {
    pub(crate) fn push_chunk(&mut self, chunk: Bytes) -> Vec<Bytes> {
        if chunk.is_empty() {
            return Vec::new();
        }
        if sse_needs_line_break(&self.pending, &chunk) {
            self.pending.push(b'\n');
        }
        self.pending.extend_from_slice(&chunk);

        let mut out = Vec::new();
        loop {
            let Some(frame_len) = sse_frame_len(&self.pending) else {
                break;
            };
            let frame = self.pending.drain(..frame_len).collect::<Vec<_>>();
            out.push(self.emit_frame(&frame));
        }

        if self.pending.iter().all(|b| b.is_ascii_whitespace()) {
            self.pending.clear();
            return out;
        }
        if !self.pending.is_empty() && sse_can_emit_without_delimiter(&self.pending) {
            let frame = std::mem::take(&mut self.pending);
            out.push(self.emit_frame(&frame));
        }

        out
    }

    pub(crate) fn finish(&mut self) -> Vec<Bytes> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        if self.pending.iter().all(|b| b.is_ascii_whitespace()) {
            self.pending.clear();
            return Vec::new();
        }
        if !sse_can_emit_without_delimiter(&self.pending) {
            self.pending.clear();
            return Vec::new();
        }
        let frame = std::mem::take(&mut self.pending);
        vec![self.emit_frame(&frame)]
    }

    fn emit_frame(&mut self, frame: &[u8]) -> Bytes {
        self.repair_frame(frame)
    }

    fn repair_frame(&mut self, frame: &[u8]) -> Bytes {
        let Some(payload) = sse_data_payload(frame) else {
            return normalize_sse_frame_bytes(frame);
        };
        if payload.is_empty() || payload == b"[DONE]" {
            return normalize_sse_frame_bytes(frame);
        }
        let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
            return normalize_sse_frame_bytes(frame);
        };

        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.output_item.done" => self.record_output_item(&value),
            "response.completed" => {
                let repaired = self.repair_completed_payload(&value);
                if repaired != value {
                    return sse_frame_with_payload(frame, &repaired);
                }
            }
            _ => {}
        }

        normalize_sse_frame_bytes(frame)
    }

    fn record_output_item(&mut self, payload: &Value) {
        let Some(item) = payload.get("item") else {
            return;
        };
        if !item.is_object()
            || item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
        {
            return;
        }
        let Ok(item_raw) = serde_json::to_vec(item) else {
            return;
        };

        if let Some(index) = payload.get("output_index").and_then(Value::as_u64) {
            self.output_items.insert(index as usize, item_raw);
            return;
        }
        self.unindexed_output_items.push(item_raw);
    }

    fn repair_completed_payload(&self, payload: &Value) -> Value {
        if self.output_items.is_empty() && self.unindexed_output_items.is_empty() {
            return payload.clone();
        }
        let has_existing_output = payload
            .get("response")
            .and_then(|response| response.get("output"))
            .map(|output| {
                !output.is_array()
                    || output
                        .as_array()
                        .map(|items| !items.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if has_existing_output {
            return payload.clone();
        }

        let mut completed = payload.clone();
        let response = completed.get_mut("response").and_then(Value::as_object_mut);
        let Some(response) = response else {
            return payload.clone();
        };

        let mut output = Vec::new();
        for item in self.output_items.values() {
            if let Ok(value) = serde_json::from_slice::<Value>(item) {
                output.push(value);
            }
        }
        for item in &self.unindexed_output_items {
            if let Ok(value) = serde_json::from_slice::<Value>(item) {
                output.push(value);
            }
        }
        response.insert("output".to_string(), Value::Array(output));
        completed
    }
}

pub(super) fn extract_completed_response_from_sse(bytes: &[u8]) -> anyhow::Result<Bytes> {
    for frame in sse_frames(bytes) {
        let Some(payload) = sse_data_payload(frame) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        if value
            .get("type")
            .and_then(Value::as_str)
            .map(|value| value == "response.completed")
            .unwrap_or(false)
        {
            let response = value.get("response").ok_or_else(|| {
                anyhow::anyhow!("response.completed event missing response payload")
            })?;
            return serde_json::to_vec(response)
                .map(Bytes::from)
                .map_err(anyhow::Error::from);
        }
    }
    anyhow::bail!("upstream stream did not produce response.completed")
}

fn sse_frames(bytes: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            frames.push(&bytes[start..index]);
            index += 4;
            start = index;
            continue;
        }
        if bytes[index..].starts_with(b"\n\n") {
            frames.push(&bytes[start..index]);
            index += 2;
            start = index;
            continue;
        }
        index += 1;
    }

    if start < bytes.len() {
        frames.push(&bytes[start..]);
    }

    frames
}

pub(crate) fn sse_data_payload(frame: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(frame).ok()?;
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(data) = trimmed.strip_prefix("data:") {
            lines.push(data.trim_start().to_string());
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n").into_bytes())
}

fn sse_frame_len(chunk: &[u8]) -> Option<usize> {
    if chunk.is_empty() {
        return None;
    }
    let lf = chunk.windows(2).position(|window| window == b"\n\n");
    let crlf = chunk.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (None, None) => None,
        (Some(pos), None) => Some(pos + 2),
        (None, Some(pos)) => Some(pos + 4),
        (Some(lf_pos), Some(crlf_pos)) => Some(if lf_pos < crlf_pos {
            lf_pos + 2
        } else {
            crlf_pos + 4
        }),
    }
}

fn sse_needs_line_break(pending: &[u8], chunk: &[u8]) -> bool {
    if pending.is_empty() || chunk.is_empty() {
        return false;
    }
    if pending.ends_with(b"\n") || pending.ends_with(b"\r") {
        return false;
    }
    if chunk[0] == b'\n' || chunk[0] == b'\r' {
        return false;
    }
    let trimmed = chunk
        .iter()
        .skip_while(|b| matches!(**b, b' ' | b'\t'))
        .copied()
        .collect::<Vec<_>>();
    if trimmed.is_empty() {
        return false;
    }
    [
        b"data:".as_slice(),
        b"event:".as_slice(),
        b"id:".as_slice(),
        b"retry:".as_slice(),
        b":".as_slice(),
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
}

fn sse_can_emit_without_delimiter(chunk: &[u8]) -> bool {
    let trimmed = trim_ascii_whitespace(chunk);
    !trimmed.is_empty()
        && sse_has_field(trimmed, b"data:")
        && !sse_needs_more_data(trimmed)
        && sse_data_lines_valid(trimmed)
}

fn sse_needs_more_data(chunk: &[u8]) -> bool {
    let trimmed = trim_ascii_whitespace(chunk);
    !trimmed.is_empty() && sse_has_field(trimmed, b"event:") && !sse_has_field(trimmed, b"data:")
}

fn sse_has_field(chunk: &[u8], prefix: &[u8]) -> bool {
    chunk
        .split(|b| *b == b'\n')
        .map(|line| trim_ascii_whitespace(trim_trailing_carriage_return(line)))
        .any(|line| line.starts_with(prefix))
}

fn sse_data_lines_valid(chunk: &[u8]) -> bool {
    for line in chunk.split(|b| *b == b'\n') {
        let line = trim_ascii_whitespace(trim_trailing_carriage_return(line));
        if line.is_empty() || !line.starts_with(b"data:") {
            continue;
        }
        let data = trim_ascii_whitespace(&line[b"data:".len()..]);
        if data.is_empty() || data == b"[DONE]" {
            continue;
        }
        if serde_json::from_slice::<Value>(data).is_err() {
            return false;
        }
    }
    true
}

fn normalize_sse_frame_bytes(frame: &[u8]) -> Bytes {
    if frame.ends_with(b"\n\n") || frame.ends_with(b"\r\n\r\n") {
        return Bytes::copy_from_slice(frame);
    }
    let mut out = Vec::from(frame);
    out.extend_from_slice(b"\n\n");
    Bytes::from(out)
}

fn sse_frame_with_payload(frame: &[u8], payload: &Value) -> Bytes {
    let mut out = String::new();
    for raw_line in frame.split(|b| *b == b'\n') {
        let line = trim_trailing_carriage_return(raw_line);
        let trimmed = trim_ascii_whitespace(line);
        if trimmed.is_empty() || trimmed.starts_with(b"data:") {
            continue;
        }
        out.push_str(std::str::from_utf8(line).unwrap_or_default());
        out.push('\n');
    }

    let payload_text = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    for line in payload_text.lines() {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    Bytes::from(out)
}

fn trim_trailing_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}
