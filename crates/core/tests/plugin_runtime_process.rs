use std::io::Read;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn helper(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_megle-core"));
    command
        .env_clear()
        .env("MEGLE_PLUGIN_RUNTIME_TEST_HELPER", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn real_helper_exit_and_malformed_stdout_are_deterministic() {
    assert!(helper("exit").status().unwrap().success());
    let output = helper("stdout-noise").output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"not-a-framed-provider");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("token"));
}

#[test]
fn real_helper_hang_is_terminable_within_deadline() {
    let mut child = helper("hang").spawn().unwrap();
    let start = Instant::now();
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn real_framed_helper_accepts_split_frame_and_returns_correlated_response() {
    let mut child = helper("framed")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let request_id = uuid::Uuid::now_v7();
    let request = serde_json::json!({
        "wireVersion":1,
        "messageType":"request",
        "requestId":request_id,
        "method":"describe",
        "deadlineMs":5000,
        "bindingEpoch":null,
        "transportGeneration":null,
        "payload":null
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let mut frame = Vec::with_capacity(17 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.push(1);
    frame.extend_from_slice(&0_u64.to_le_bytes());
    frame.extend_from_slice(&0_u32.to_le_bytes());
    frame.extend_from_slice(&payload);
    let stdin = child.stdin.as_mut().unwrap();
    stdin.write_all(&frame[..7]).unwrap();
    stdin.flush().unwrap();
    stdin.write_all(&frame[7..]).unwrap();
    stdin.flush().unwrap();

    let stdout = child.stdout.as_mut().unwrap();
    let mut header = [0_u8; 17];
    stdout.read_exact(&mut header).unwrap();
    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let mut response = vec![0_u8; size];
    stdout.read_exact(&mut response).unwrap();
    let response: serde_json::Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["requestId"], request_id.to_string());
    assert_eq!(response["messageType"], "response");
    assert_eq!(response["payload"]["pluginId"], "com.megle.fake");
    child.kill().unwrap();
    let _ = child.wait().unwrap();
}

#[test]
fn real_helper_tree_can_be_terminated_as_a_process_tree() {
    let mut child = helper("tree").spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut line = String::new();
    loop {
        let mut byte = [0_u8; 1];
        stdout.read_exact(&mut byte).unwrap();
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0] as char);
    }
    let descendant: u32 = line.trim().parse().unwrap();
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
            libc::kill(descendant as i32, libc::SIGKILL);
        }
    }
    let _ = child.wait().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    #[cfg(windows)]
    assert!(!Command::new("tasklist")
        .args(["/FI", &format!("PID eq {descendant}"), "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&descendant.to_string()))
        .unwrap_or(false));
}
