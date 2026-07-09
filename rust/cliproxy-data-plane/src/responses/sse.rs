use bytes::Bytes;
use serde_json::Value;

use super::protocol::ResponsesStreamEventIr;

/// 增量式 SSE 分帧器，以及当前 `/v1/responses` 路径对齐 CPA 所需的最小修复逻辑。
#[derive(Debug, Default)]
pub(super) struct ResponsesSseFramer {
    pending: Vec<u8>,
    // 有些上游会把 `event:` 和 `data:` 拆到不同 chunk 里，这里先暂存头部，
    // 等后续 data frame 到达后再合并。
    pending_header: Option<Vec<u8>>,
    // 记录已经完成的 output item，这样当终态 `response.completed`
    // 没有带完整 `response.output` 时，可以按前序事件补齐。
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
        while let Some(frame_len) = sse_frame_len(&self.pending) {
            let frame = self.pending.drain(..frame_len).collect::<Vec<_>>();
            if let Some(frame) = self.consume_frame(frame) {
                out.push(frame);
            }
        }

        if self.pending.iter().all(|b| b.is_ascii_whitespace()) {
            self.pending.clear();
            return out;
        }
        if !self.pending.is_empty() && sse_can_emit_without_delimiter(&self.pending) {
            let frame = std::mem::take(&mut self.pending);
            if let Some(frame) = self.consume_frame(frame) {
                out.push(frame);
            }
        }

        out
    }

    pub(crate) fn finish(&mut self) -> Vec<Bytes> {
        self.pending_header = None;
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
        self.consume_frame(frame).into_iter().collect()
    }

    fn emit_frame(&mut self, frame: &[u8]) -> Bytes {
        self.repair_frame(frame)
    }

    fn consume_frame(&mut self, frame: Vec<u8>) -> Option<Bytes> {
        if is_header_only_sse_frame(&frame) {
            self.pending_header = Some(trim_sse_frame_terminator(&frame).to_vec());
            return None;
        }

        let merged = if let Some(header) = self.pending_header.take() {
            if sse_has_field(&frame, b"data:") && !sse_has_field(&frame, b"event:") {
                merge_sse_header_and_frame(&header, &frame)
            } else {
                frame
            }
        } else {
            frame
        };

        Some(self.emit_frame(&merged))
    }

    fn repair_frame(&mut self, frame: &[u8]) -> Bytes {
        let event = match ResponsesStreamEventIr::from_sse_frame(frame) {
            Ok(Some(event)) => event,
            Ok(None) => return normalize_sse_frame_bytes(frame),
            Err(_) => return normalize_sse_frame_bytes(frame),
        };

        match event {
            ResponsesStreamEventIr::OutputItemDone(done) => {
                self.record_output_item(&done.item, done.output_index);
            }
            ResponsesStreamEventIr::Completed(completed) => {
                // Go 侧已经会基于前面见过的 output-item 事件修复终态 completed，
                // 这里镜像同样的行为。
                let repaired = self.repair_completed_response(&completed.response);
                if repaired != completed.response {
                    let mut payload = completed.payload;
                    if let Some(response) = payload.get_mut("response") {
                        *response = repaired;
                    }
                    return sse_frame_with_payload(frame, &payload);
                }
            }
            ResponsesStreamEventIr::Done => return normalize_sse_frame_bytes(frame),
            ResponsesStreamEventIr::OtherJson(_) | ResponsesStreamEventIr::NonJson(_) => {}
        }

        let Some(payload) = sse_data_payload(frame) else {
            return normalize_sse_frame_bytes(frame);
        };
        if payload.is_empty() {
            return normalize_sse_frame_bytes(frame);
        }

        normalize_sse_frame_bytes(frame)
    }

    fn record_output_item(&mut self, item: &Value, output_index: Option<usize>) {
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

        if let Some(index) = output_index {
            self.output_items.insert(index, item_raw);
            return;
        }
        self.unindexed_output_items.push(item_raw);
    }

    fn repair_completed_response(&self, response: &Value) -> Value {
        if self.output_items.is_empty() && self.unindexed_output_items.is_empty() {
            return response.clone();
        }
        let has_existing_output = response
            .get("output")
            .map(|output| {
                !output.is_array()
                    || output
                        .as_array()
                        .map(|items| !items.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if has_existing_output {
            return response.clone();
        }

        let mut repaired = response.clone();
        let response = repaired.as_object_mut();
        let Some(response) = response else {
            return repaired;
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
        repaired
    }
}

/// 在完成分帧和修复后，从整段 SSE transcript 中提取终态 response JSON。
pub(super) fn extract_completed_response_from_sse(bytes: &[u8]) -> anyhow::Result<Bytes> {
    for frame in sse_frames(bytes) {
        let Some(event) = ResponsesStreamEventIr::from_sse_frame(frame)? else {
            continue;
        };
        if let ResponsesStreamEventIr::Completed(completed) = event {
            return serde_json::to_vec(&completed.response)
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

fn is_header_only_sse_frame(frame: &[u8]) -> bool {
    let trimmed = trim_ascii_whitespace(frame);
    !trimmed.is_empty()
        && !sse_has_field(trimmed, b"data:")
        && (sse_has_field(trimmed, b"event:")
            || sse_has_field(trimmed, b"id:")
            || sse_has_field(trimmed, b"retry:")
            || sse_has_field(trimmed, b":"))
}

fn trim_sse_frame_terminator(frame: &[u8]) -> &[u8] {
    frame
        .strip_suffix(b"\r\n\r\n")
        .or_else(|| frame.strip_suffix(b"\n\n"))
        .unwrap_or(frame)
}

fn merge_sse_header_and_frame(header: &[u8], frame: &[u8]) -> Vec<u8> {
    let mut merged = trim_sse_frame_terminator(header).to_vec();
    if !merged.ends_with(b"\n") && !frame.starts_with(b"\n") && !frame.starts_with(b"\r") {
        merged.push(b'\n');
    }
    merged.extend_from_slice(frame);
    merged
}
