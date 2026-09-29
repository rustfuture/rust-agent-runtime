//! Line format of the append-only event log: task state lines and the
//! metadata-only trace lines. Traces record counts and sizes, never argument
//! values or command output.

use crate::{ExecutionTrace, State};

pub(crate) fn parse_trace(line: &str) -> Option<(String, ExecutionTrace)> {
    let mut parts = line.split('\t');
    if parts.next()? != "trace" {
        return None;
    }
    parse_trace_fields(parts)
}

pub(crate) fn parse_tool_trace(line: &str) -> Option<(String, ExecutionTrace)> {
    let mut parts = line.split('\t');
    if parts.next()? != "tool" {
        return None;
    }
    parse_trace_fields(parts)
}

fn parse_trace_fields<'a>(
    mut parts: impl Iterator<Item = &'a str>,
) -> Option<(String, ExecutionTrace)> {
    let id = parts.next()?.to_owned();
    let attempt = parts.next()?.parse().ok()?;
    let program = parts.next()?.to_owned();
    let argument_count = parts.next()?.parse().ok()?;
    let status_text = parts.next()?;
    let status = if status_text == "none" {
        None
    } else {
        Some(status_text.parse().ok()?)
    };
    let timed_out = parts.next()?.parse().ok()?;
    let cancelled = parts.next()?.parse().ok()?;
    let output_truncated = parts.next()?.parse().ok()?;
    let stdout_bytes = parts.next()?.parse().ok()?;
    let stderr_bytes = parts.next()?.parse().ok()?;
    let duration_ms = parts.next()?.parse().ok()?;
    Some((
        id,
        ExecutionTrace {
            attempt,
            program,
            argument_count,
            status,
            timed_out,
            cancelled,
            output_truncated,
            stdout_bytes,
            stderr_bytes,
            duration_ms,
        },
    ))
}

pub(crate) fn trace_state(trace: &ExecutionTrace) -> State {
    if trace.cancelled {
        State::Cancelled
    } else if trace.status == Some(0) && !trace.timed_out {
        State::Succeeded
    } else {
        State::Failed
    }
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}
