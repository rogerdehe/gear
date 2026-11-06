//! T-640: the real door sends `workspace.close` with the workspace id.
//!
//! A fake Herdr socket answers the protocol check and records the request the
//! client sends, so the test reads the method and params off the wire.

use gear_herdr::{HerdrAct, SocketHerdr};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::Duration;

/// A socket path in the test's scratch directory, short enough for AF_UNIX.
fn socket_path() -> PathBuf {
    let dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(dir).join(format!("gear-herdr-ws-{}.sock", std::process::id()))
}

#[test]
fn the_real_client_sends_workspace_close_with_the_workspace_id() {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind the fake Herdr socket");
    listener.set_nonblocking(true).expect("non-blocking accept");

    // The server answers the protocol check and stops once it has seen the
    // `workspace.close` request, handing the captured request back.
    let server = std::thread::spawn(move || {
        let mut captured: Option<(String, serde_json::Value)> = None;
        while captured.is_none() {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking connection");
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        // The connect probe: no request.
                        continue;
                    }
                    let request: serde_json::Value =
                        serde_json::from_str(line.trim()).expect("a request line");
                    let method = request["method"].as_str().unwrap_or_default().to_string();
                    let result = if method == "ping" {
                        serde_json::json!({"version": "0.9.0", "protocol": 22})
                    } else {
                        serde_json::json!({})
                    };
                    let reply = serde_json::json!({"id": request["id"], "result": result});
                    let mut writer = stream;
                    writer
                        .write_all(format!("{reply}\n").as_bytes())
                        .expect("write the reply");
                    writer.flush().expect("flush the reply");
                    if method == "workspace.close" {
                        captured = Some((method, request["params"].clone()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("accept failed: {e}"),
            }
        }
        captured
    });

    let mut herdr = SocketHerdr::connect(&path).expect("connect to the fake Herdr");
    assert_eq!(herdr.workspace_close("w1KM"), gear_herdr::Outcome::Ok(()));
    let captured = server.join().expect("the fake Herdr finishes");
    let _ = std::fs::remove_file(&path);
    let (method, params) = captured.expect("the client sent workspace.close");
    assert_eq!(method, "workspace.close");
    assert_eq!(params, serde_json::json!({"workspace_id": "w1KM"}));
}
