use std::fmt::Write;

const FIELD_DELIM: char = '\t';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Next,
    Prev,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Suggest {
        request_id: u64,
        buffer: String,
        cursor: usize,
        cwd: String,
        session_id: String,
        last_command: Option<String>,
    },
    Cycle {
        session_id: String,
        direction: CycleDirection,
    },
    Feedback {
        command: String,
        source: String,
        score: f32,
        accepted: bool,
        session_id: String,
    },
    Record {
        command: String,
        cwd: String,
        exit_code: i32,
        duration_ms: u64,
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DaemonMessage {
    Suggestion {
        request_id: u64,
        candidate_count: usize,
        candidate_index: usize,
        source: String,
        score: f32,
        text: String,
    },
    Ack {
        request_id: u64,
    },
    Error {
        message: String,
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    message: String,
}

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode_client_message(message: &ClientMessage) -> String {
    let mut line = String::new();
    match message {
        ClientMessage::Suggest {
            request_id,
            buffer,
            cursor,
            cwd,
            session_id,
            last_command,
        } => {
            append_tag(&mut line, "s");
            append_u64(&mut line, *request_id);
            append_usize(&mut line, *cursor);
            append_escaped(&mut line, session_id);
            append_escaped(&mut line, cwd);
            append_escaped(&mut line, buffer);
            append_escaped(&mut line, last_command.as_deref().unwrap_or(""));
        }
        ClientMessage::Cycle {
            session_id,
            direction,
        } => {
            append_tag(&mut line, "c");
            append_escaped(&mut line, session_id);
            append_tag(
                &mut line,
                match direction {
                    CycleDirection::Next => "n",
                    CycleDirection::Prev => "p",
                },
            );
        }
        ClientMessage::Feedback {
            command,
            source,
            score,
            accepted,
            session_id,
        } => {
            append_tag(&mut line, "f");
            append_tag(&mut line, if *accepted { "1" } else { "0" });
            append_f32(&mut line, *score);
            append_escaped(&mut line, session_id);
            append_escaped(&mut line, source);
            append_escaped(&mut line, command);
        }
        ClientMessage::Record {
            command,
            cwd,
            exit_code,
            duration_ms,
            session_id,
        } => {
            append_tag(&mut line, "r");
            append_i32(&mut line, *exit_code);
            append_u64(&mut line, *duration_ms);
            append_escaped(&mut line, session_id);
            append_escaped(&mut line, cwd);
            append_escaped(&mut line, command);
        }
    }
    line
}

pub fn parse_client_message(line: &str) -> Result<ClientMessage, ProtocolError> {
    let fields = split_fields(line);
    let Some(tag) = fields.first().copied() else {
        return Err(ProtocolError::new("empty message"));
    };

    match tag {
        "s" => {
            require_fields(&fields, 6, "suggest")?;
            Ok(ClientMessage::Suggest {
                request_id: parse_u64(fields[1], "request_id")?,
                cursor: parse_usize(fields[2], "cursor")?,
                session_id: unescape_field(fields[3])?,
                cwd: unescape_field(fields[4])?,
                buffer: unescape_field(fields[5])?,
                last_command: fields
                    .get(6)
                    .map(|value| unescape_field(value))
                    .transpose()?
                    .filter(|value| !value.is_empty()),
            })
        }
        "c" => {
            require_fields(&fields, 3, "cycle")?;
            Ok(ClientMessage::Cycle {
                session_id: unescape_field(fields[1])?,
                direction: parse_direction(fields[2])?,
            })
        }
        "f" => {
            require_fields(&fields, 6, "feedback")?;
            Ok(ClientMessage::Feedback {
                accepted: parse_bool(fields[1], "accepted")?,
                score: parse_f32(fields[2], "score")?,
                session_id: unescape_field(fields[3])?,
                source: unescape_field(fields[4])?,
                command: unescape_field(fields[5])?,
            })
        }
        "r" => {
            require_fields(&fields, 6, "record")?;
            Ok(ClientMessage::Record {
                exit_code: parse_i32(fields[1], "exit_code")?,
                duration_ms: parse_u64(fields[2], "duration_ms")?,
                session_id: unescape_field(fields[3])?,
                cwd: unescape_field(fields[4])?,
                command: unescape_field(fields[5])?,
            })
        }
        other => Err(ProtocolError::new(format!("unknown client message type: {other}"))),
    }
}

pub fn encode_daemon_message(message: &DaemonMessage) -> String {
    let mut line = String::new();
    match message {
        DaemonMessage::Suggestion {
            request_id,
            candidate_count,
            candidate_index,
            source,
            score,
            text,
        } => {
            append_tag(&mut line, "s");
            append_u64(&mut line, *request_id);
            append_usize(&mut line, *candidate_count);
            append_usize(&mut line, *candidate_index);
            append_f32(&mut line, *score);
            append_escaped(&mut line, source);
            append_escaped(&mut line, text);
        }
        DaemonMessage::Ack { request_id } => {
            append_tag(&mut line, "a");
            append_u64(&mut line, *request_id);
        }
        DaemonMessage::Error {
            message,
            request_id,
        } => {
            append_tag(&mut line, "e");
            append_u64(&mut line, *request_id);
            append_escaped(&mut line, message);
        }
    }
    line
}

pub fn parse_daemon_message(line: &str) -> Result<DaemonMessage, ProtocolError> {
    let fields = split_fields(line);
    let Some(tag) = fields.first().copied() else {
        return Err(ProtocolError::new("empty message"));
    };

    match tag {
        "s" => {
            require_fields(&fields, 7, "suggestion")?;
            Ok(DaemonMessage::Suggestion {
                request_id: parse_u64(fields[1], "request_id")?,
                candidate_count: parse_usize(fields[2], "candidate_count")?,
                candidate_index: parse_usize(fields[3], "candidate_index")?,
                score: parse_f32(fields[4], "score")?,
                source: unescape_field(fields[5])?,
                text: unescape_field(fields[6])?,
            })
        }
        "a" => {
            require_fields(&fields, 2, "ack")?;
            Ok(DaemonMessage::Ack {
                request_id: parse_u64(fields[1], "request_id")?,
            })
        }
        "e" => {
            require_fields(&fields, 3, "error")?;
            Ok(DaemonMessage::Error {
                request_id: parse_u64(fields[1], "request_id")?,
                message: unescape_field(fields[2])?,
            })
        }
        other => Err(ProtocolError::new(format!("unknown daemon message type: {other}"))),
    }
}

fn split_fields(line: &str) -> Vec<&str> {
    line.split(FIELD_DELIM).collect()
}

fn require_fields(fields: &[&str], expected: usize, message_type: &str) -> Result<(), ProtocolError> {
    if fields.len() < expected {
        Err(ProtocolError::new(format!(
            "{message_type} message expected at least {expected} fields, got {}",
            fields.len()
        )))
    } else {
        Ok(())
    }
}

fn parse_direction(value: &str) -> Result<CycleDirection, ProtocolError> {
    match value {
        "n" => Ok(CycleDirection::Next),
        "p" => Ok(CycleDirection::Prev),
        other => Err(ProtocolError::new(format!("invalid direction: {other}"))),
    }
}

fn parse_bool(value: &str, field: &str) -> Result<bool, ProtocolError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(ProtocolError::new(format!("invalid {field}: {other}"))),
    }
}

fn parse_u64(value: &str, field: &str) -> Result<u64, ProtocolError> {
    value
        .parse()
        .map_err(|_| ProtocolError::new(format!("invalid {field}: {value}")))
}

fn parse_usize(value: &str, field: &str) -> Result<usize, ProtocolError> {
    value
        .parse()
        .map_err(|_| ProtocolError::new(format!("invalid {field}: {value}")))
}

fn parse_i32(value: &str, field: &str) -> Result<i32, ProtocolError> {
    value
        .parse()
        .map_err(|_| ProtocolError::new(format!("invalid {field}: {value}")))
}

fn parse_f32(value: &str, field: &str) -> Result<f32, ProtocolError> {
    value
        .parse()
        .map_err(|_| ProtocolError::new(format!("invalid {field}: {value}")))
}

fn append_tag(line: &mut String, tag: &str) {
    if !line.is_empty() {
        line.push(FIELD_DELIM);
    }
    line.push_str(tag);
}

fn append_escaped(line: &mut String, value: &str) {
    if !line.is_empty() {
        line.push(FIELD_DELIM);
    }
    escape_field_into(line, value);
}

fn append_u64(line: &mut String, value: u64) {
    append_numeric(line, value);
}

fn append_usize(line: &mut String, value: usize) {
    append_numeric(line, value);
}

fn append_i32(line: &mut String, value: i32) {
    append_numeric(line, value);
}

fn append_f32(line: &mut String, value: f32) {
    append_numeric(line, value);
}

fn append_numeric<T: std::fmt::Display>(line: &mut String, value: T) {
    if !line.is_empty() {
        line.push(FIELD_DELIM);
    }
    let _ = write!(line, "{value}");
}

fn escape_field_into(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
}

fn unescape_field(value: &str) -> Result<String, ProtocolError> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err(ProtocolError::new("unterminated escape sequence"));
        };

        match escaped {
            '\\' => out.push('\\'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            other => {
                return Err(ProtocolError::new(format!(
                    "unsupported escape sequence: \\{other}"
                )))
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggest_message_roundtrip() {
        let msg = ClientMessage::Suggest {
            request_id: 42,
            buffer: "cd sr".into(),
            cursor: 5,
            cwd: "/Users/me/project".into(),
            session_id: "abc123".into(),
            last_command: None,
        };
        let line = encode_client_message(&msg);
        let parsed = parse_client_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_record_message_roundtrip() {
        let msg = ClientMessage::Record {
            command: "cd src".into(),
            cwd: "/Users/me/project".into(),
            exit_code: 0,
            duration_ms: 12,
            session_id: "abc123".into(),
        };
        let line = encode_client_message(&msg);
        let parsed = parse_client_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_cycle_message_roundtrip() {
        let msg = ClientMessage::Cycle {
            session_id: "abc123".into(),
            direction: CycleDirection::Next,
        };
        let line = encode_client_message(&msg);
        let parsed = parse_client_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_feedback_message_roundtrip() {
        let msg = ClientMessage::Feedback {
            command: "echo hello".into(),
            source: "history".into(),
            score: 0.75,
            accepted: true,
            session_id: "abc123".into(),
        };
        let line = encode_client_message(&msg);
        let parsed = parse_client_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_suggestion_response_roundtrip() {
        let msg = DaemonMessage::Suggestion {
            request_id: 42,
            candidate_count: 3,
            candidate_index: 1,
            source: "cwd_history".into(),
            score: 0.91,
            text: "cd src/components".into(),
        };
        let line = encode_daemon_message(&msg);
        let parsed = parse_daemon_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_protocol_escapes_special_characters() {
        let msg = ClientMessage::Suggest {
            request_id: 7,
            buffer: "echo one\\ttwo\nthree\tfour".into(),
            cursor: 5,
            cwd: "/home".into(),
            session_id: "s1".into(),
            last_command: Some("printf '\\n'".into()),
        };
        let line = encode_client_message(&msg);
        let parsed = parse_client_message(&line).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_parse_rejects_unknown_escape() {
        let err = parse_daemon_message("e\t0\tbad\\xescape").unwrap_err();
        assert!(err.to_string().contains("unsupported escape sequence"));
    }
}
