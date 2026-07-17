use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codex-image")
}

fn spawn_error_server() -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        sender.send(request).unwrap();
        let body = br#"{"error":"intentional test failure"}"#;
        write!(
            stream,
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    (format!("http://{address}"), receiver)
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header_end = header_end + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if request.len() >= header_end + content_length {
            break;
        }
    }
    request
}

#[test]
fn complete_no_auth_overrides_skip_codex_config_and_preserve_prompt() {
    let (server, captured_request) = spawn_error_server();
    let codex_home = tempfile::tempdir().unwrap().path().join("missing");
    let prompt = "quoted \"value\" and trailing\\path";
    let output = Command::new(binary())
        .env("CODEX_IMAGE_CLI_TEST_PROMPT", prompt)
        .arg("generate")
        .arg("--prompt-env")
        .arg("CODEX_IMAGE_CLI_TEST_PROMPT")
        .arg("--base-url")
        .arg(format!("{server}/v1"))
        .arg("--no-auth")
        .arg("--codex-home")
        .arg(codex_home)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 400 Bad Request"), "{stderr}");
    assert!(!stderr.contains("read Codex config"), "{stderr}");

    let request = captured_request
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let request_text = String::from_utf8_lossy(&request);
    assert!(request_text.starts_with("POST /v1/images/generations "));
    assert!(!request_text.to_ascii_lowercase().contains("authorization:"));
    let body_start = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    let body: serde_json::Value = serde_json::from_slice(&request[body_start..]).unwrap();
    assert_eq!(body["prompt"], prompt);
}

#[test]
fn rejects_image_counts_above_the_billing_limit() {
    let output = Command::new(binary())
        .args(["generate", "--prompt", "test", "--n", "11"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("1..=10"), "{stderr}");
}
