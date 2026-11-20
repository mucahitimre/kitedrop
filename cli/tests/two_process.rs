//! Integration test between two processes (plan Phase 1 exit criterion): two `kitedrop daemon`
//! processes on loopback pair, exchange text and a file, and a send to a stopped process is
//! delivered after it restarts.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Daemon {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    seen: Vec<Value>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Daemon {
    fn start(dir: &Path, name: &str, port: u16) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kitedrop"))
            .args(["daemon", "--dir"])
            .arg(dir)
            .args([
                "--name",
                name,
                "--bind",
                "127.0.0.1",
                "--no-mdns",
                "--no-scan",
            ])
            .args(["--port", &port.to_string()])
            .args(["--advertise", &format!("127.0.0.1:{port}")])
            .args(["--presence", "5", "--retry", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let _ = tx.send(v);
                }
            }
        });
        let mut d = Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
        };
        d.wait("ready", 10, |v| v["ready"] == true);
        d
    }

    /// Waits for a line matching `f`, keeping every line seen for later checks.
    fn wait(&mut self, what: &str, secs: u64, f: impl Fn(&Value) -> bool) -> Value {
        if let Some(v) = self.seen.iter().find(|v| f(v)) {
            return v.clone();
        }
        let end = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(v) => {
                    self.seen.push(v.clone());
                    if f(&v) {
                        return v;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("timed out waiting for {what}; seen: {:#?}", self.seen);
    }

    fn cmd(&mut self, line: &str) -> Value {
        self.seen.clear();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let v = self.wait(line, 20, |v| {
            v.get("ok").is_some() || v.get("error").is_some()
        });
        assert!(v.get("error").is_none(), "{line}: {v}");
        v["ok"].clone()
    }

    fn stop(mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn delivered(d: &mut Daemon, tid: &str, secs: u64) {
    d.wait("delivered", secs, |v| {
        v["event"] == "transfer"
            && v["transfer"]["transfer_id"] == tid
            && v["transfer"]["state"] == "delivered"
    });
}

#[test]
fn two_processes_pair_send_and_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let (pa, pb) = (free_port(), free_port());
    let mut a = Daemon::start(&tmp.path().join("a"), "mac", pa);
    let mut b = Daemon::start(&tmp.path().join("b"), "pixel", pb);

    let uri = a.cmd("invite")["uri"].as_str().unwrap().to_owned();
    b.cmd(&format!("pair {uri}"));
    assert_eq!(b.cmd("devices")["devices"].as_array().unwrap().len(), 2);

    // Text B → A.
    let tid = b.cmd("send-text mac hello across processes")["transfers"][0]
        .as_str()
        .unwrap()
        .to_owned();
    delivered(&mut b, &tid, 15);
    a.wait("text event", 5, |v| {
        v["event"] == "text" && v["text"] == "hello across processes"
    });

    // File A → B, multi-chunk.
    let file = tmp.path().join("report.pdf");
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    let tid = a.cmd(&format!("send-file pixel {}", file.display()))["transfers"][0]
        .as_str()
        .unwrap()
        .to_owned();
    delivered(&mut a, &tid, 15);
    let ev = b.wait("file event", 5, |v| v["event"] == "file");
    let path = ev["path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);

    // A stops; B's send waits, then is delivered when A comes back.
    a.stop();
    let tid = b.cmd("send-text mac after restart")["transfers"][0]
        .as_str()
        .unwrap()
        .to_owned();
    b.wait("waiting", 15, |v| {
        v["event"] == "transfer"
            && v["transfer"]["transfer_id"] == tid.as_str()
            && v["transfer"]["state"] == "waiting"
    });
    let mut a = Daemon::start(&tmp.path().join("a"), "mac", pa);
    delivered(&mut b, &tid, 30);
    a.wait("text after restart", 5, |v| {
        v["event"] == "text" && v["text"] == "after restart"
    });
    // State survived the restart: A still knows B.
    assert_eq!(a.cmd("devices")["devices"].as_array().unwrap().len(), 2);
    a.stop();
    b.stop();
}
