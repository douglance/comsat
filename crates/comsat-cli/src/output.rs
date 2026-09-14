//! Convert Incurs transport envelopes to COMSAT's Unix data stream.
use std::io::{self, Write};

use serde_json::Value;

const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

pub struct UnixOutput<W, E> {
    stdout: W,
    stderr: E,
    pending: Vec<u8>,
    invalid_invocation: bool,
}

impl<W: Write, E: Write> UnixOutput<W, E> {
    pub const fn new(stdout: W, stderr: E) -> Self {
        Self {
            stdout,
            stderr,
            pending: Vec::new(),
            invalid_invocation: false,
        }
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            self.emit_line()?;
        }
        if !self.pending.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete JSON output",
            ));
        }
        self.flush()
    }

    pub const fn invalid_invocation(&self) -> bool {
        self.invalid_invocation
    }

    fn emit_line(&mut self) -> io::Result<()> {
        let line = std::mem::take(&mut self.pending);
        let value = match serde_json::from_slice::<Value>(&line) {
            Ok(value) => value,
            Err(error) => return self.emit_unparsed(line, &error),
        };
        self.emit_value(&value)
    }

    fn emit_value(&mut self, value: &Value) -> io::Result<()> {
        if self.emit_error(value)? {
            return Ok(());
        }
        match value.get("type").and_then(Value::as_str) {
            Some("chunk") => self.emit_data(&value["data"]),
            Some("error") => write_json_line(&mut self.stderr, value),
            Some("done") => Ok(()),
            _ if value.get("ok") == Some(&Value::Bool(false)) => {
                write_json_line(&mut self.stderr, value)
            }
            _ => self.emit_data(value),
        }
    }

    fn emit_unparsed(&mut self, mut line: Vec<u8>, error: &serde_json::Error) -> io::Result<()> {
        let first = line.iter().find(|byte| !byte.is_ascii_whitespace());
        if error.is_eof() && matches!(first, Some(b'{' | b'[')) {
            line.push(b'\n');
            self.pending = line;
            return Ok(());
        }
        self.stdout.write_all(&line)?;
        self.stdout.write_all(b"\n")
    }

    fn emit_error(&mut self, value: &Value) -> io::Result<bool> {
        let error = value.get("error").unwrap_or(value);
        let Some(code) = error.get("code").and_then(Value::as_str) else {
            return Ok(false);
        };
        self.invalid_invocation |= matches!(
            code,
            "VALIDATION_ERROR"
                | "UNKNOWN_COMMAND"
                | "COMMAND_NOT_FOUND"
                | "UNKNOWN_OPTION"
                | "PARSE_ERROR"
        );
        if error.get("message").is_none() || value.get("id").is_some() {
            return Ok(false);
        }
        write_json_line(&mut self.stderr, value)?;
        Ok(true)
    }

    fn emit_data(&mut self, data: &Value) -> io::Result<()> {
        if data.get("type").and_then(Value::as_str) == Some("source_error") {
            return write_json_line(&mut self.stderr, &data["diagnostic"]);
        }
        write_json_line(&mut self.stdout, data)
    }
}

impl<W: Write, E: Write> Write for UnixOutput<W, E> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        for &byte in input {
            if byte == b'\n' {
                self.emit_line()?;
                continue;
            }
            if self.pending.len() >= MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "output line exceeds 8 MiB",
                ));
            }
            self.pending.push(byte);
        }
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()?;
        self.stderr.flush()
    }
}

fn write_json_line(output: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::UnixOutput;
    use serde_json::{Value, json};
    use std::io::Write;

    #[test]
    fn fragmented_chunks_become_plain_records_and_diagnostics_go_to_stderr() {
        let record = json!({"id":"1","source":"github","text":"one\ntwo"});
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type":"chunk","data":record}),
            json!({"type":"chunk","data":{"type":"source_error","diagnostic":{"source":"web","class":"authentication"}}}),
            json!({"type":"error","error":{"code":"partial"}}),
            json!({"type":"done","ok":true})
        );
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut writer = UnixOutput::new(&mut out, &mut err);
        for fragment in input.as_bytes().chunks(7) {
            writer.write_all(fragment).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&out).unwrap(), record);
        let diagnostics = String::from_utf8(err).unwrap();
        assert_eq!(diagnostics.lines().count(), 2);
        assert!(diagnostics.contains("authentication"));
    }

    #[test]
    fn human_help_is_preserved() {
        let mut output = Vec::new();
        let mut writer = UnixOutput::new(&mut output, Vec::new());
        writer.write_all(b"Usage: comsat search\n").unwrap();
        writer.finish().unwrap();
        assert_eq!(output, b"Usage: comsat search\n");
    }

    #[test]
    fn bare_validation_errors_are_diagnostics_and_mark_invalid_invocation() {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut writer = UnixOutput::new(&mut output, &mut errors);
        writer
            .write_all(b"{\"code\":\"VALIDATION_ERROR\",\"message\":\"missing text\"}\n")
            .unwrap();
        writer.finish().unwrap();
        assert!(writer.invalid_invocation());
        assert!(output.is_empty());
        assert!(String::from_utf8(errors).unwrap().contains("missing text"));
    }

    #[test]
    fn pretty_json_validation_errors_stay_off_stdout() {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut writer = UnixOutput::new(&mut output, &mut errors);
        let error = json!({"code":"VALIDATION_ERROR","message":"missing text"});
        writer
            .write_all(serde_json::to_string_pretty(&error).unwrap().as_bytes())
            .unwrap();
        writer.finish().unwrap();
        assert!(writer.invalid_invocation());
        assert!(output.is_empty());
        assert_eq!(serde_json::from_slice::<Value>(&errors).unwrap(), error);
    }

    #[test]
    fn command_not_found_marks_invalid_invocation() {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut writer = UnixOutput::new(&mut output, &mut errors);
        writer
            .write_all(
                b"{\"code\":\"COMMAND_NOT_FOUND\",\"message\":\"'blorp' is not a command for 'comsat'.\"}\n",
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(writer.invalid_invocation());
        assert!(output.is_empty());
        assert!(
            String::from_utf8(errors)
                .unwrap()
                .contains("COMMAND_NOT_FOUND")
        );
    }
}
