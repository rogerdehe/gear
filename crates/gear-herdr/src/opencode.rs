//! Bounded OpenCode service transport. Arguments never pass through a shell.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn request(args: &[String]) -> Result<serde_json::Value, String> {
    let mut child = Command::new("opencode")
        .arg("api")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("OpenCode API: {e}"))?;
    // model.list can exceed a pipe buffer: drain both streams while waiting.
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("OpenCode API timeout or wait error: {other:?}"));
            }
        }
    };
    let stdout = out
        .join()
        .map_err(|_| "API stdout reader failed")?
        .map_err(|e| e.to_string())?;
    let stderr = err
        .join()
        .map_err(|_| "API stderr reader failed")?
        .map_err(|e| e.to_string())?;
    let status = status?;
    if !status.success() {
        return Err(format!(
            "OpenCode API {status}: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    decode(&stdout)
}

fn decode(stdout: &[u8]) -> Result<serde_json::Value, String> {
    // switchModel succeeds with HTTP 204 and the CLI prints no body.
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(stdout).map_err(|e| format!("OpenCode API JSON: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_successful_no_content_response_is_not_an_api_failure() {
        assert_eq!(super::decode(b"\n").unwrap(), serde_json::Value::Null);
        assert!(super::decode(b"not JSON").is_err());
    }
}
