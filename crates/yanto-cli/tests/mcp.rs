//! `yanto mcp` over stdio, end to end: spawn the real binary against a temp data dir, speak
//! newline-delimited JSON-RPC, and check authentication, tool listing and a read/write call.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};
use yanto_core::actor::Scope;
use yanto_core::testing::TestCore;

struct Session {
    child: Child,
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    fn spawn(config: &Path, env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_yanto"));
        cmd.arg("mcp")
            .arg("--config")
            .arg(config)
            .env_remove("YANTO_CONFIG")
            .env_remove("YANTO_API_TOKEN")
            .env_remove("YANTO_API_TOKEN_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn yanto mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines: rx,
            next_id: 1,
        }
    }

    fn send(&mut self, v: &Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("a JSON-RPC response on stdout");
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout must carry only JSON-RPC ({e}): {line}"));
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        let r = self.request(
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}),
        );
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        r
    }

    fn finish(mut self) -> (std::process::ExitStatus, String) {
        drop(self.child.stdin.take());
        let status = self.child.wait().unwrap();
        let mut err = String::new();
        std::io::Read::read_to_string(&mut self.child.stderr.take().unwrap(), &mut err).unwrap();
        (status, err)
    }
}

fn write_config(core: &TestCore) -> std::path::PathBuf {
    let path = core.dir.path().join("mcp-config.toml");
    std::fs::write(
        &path,
        format!(
            "config_version = 1\n[paths]\ndata_dir = \"{}\"\n",
            core.dir.path().display()
        ),
    )
    .unwrap();
    path
}

async fn setup(scope: Scope) -> (TestCore, std::path::PathBuf, String) {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let (token, _) = core.token_actor(&owner, scope).await;
    let cfg = write_config(&core);
    (core, cfg, token)
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_round_trip_initialize_list_and_call() {
    let (_core, cfg, token) = setup(Scope::Write).await;
    let out = tokio::task::spawn_blocking(move || {
        let mut s = Session::spawn(&cfg, &[("YANTO_API_TOKEN", &token)]);
        let init = s.initialize();
        let list = s.request("tools/list", json!({}));
        let who = s.request(
            "tools/call",
            json!({"name":"yanto_whoami","arguments":{}}),
        );
        let create = s.request(
            "tools/call",
            json!({"name":"yanto_project_create","arguments":{"name":"Std","image_ref":"nginx:alpine"}}),
        );
        let del = s.request(
            "tools/call",
            json!({"name":"yanto_project_delete","arguments":{"id":"x","confirm":true}}),
        );
        let (status, stderr) = s.finish();
        (init, list, who, create, del, status, stderr)
    })
    .await
    .unwrap();
    let (init, list, who, create, del, status, stderr) = out;
    assert_eq!(init["result"]["serverInfo"]["name"], "yanto");
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"yanto_projects_list"));
    assert!(
        names.contains(&"yanto_project_create"),
        "write scope lists write tools"
    );
    assert!(
        !names.contains(&"yanto_project_delete"),
        "admin tools are not listed to write"
    );
    assert!(!names.iter().any(|n| n.contains("exec")));
    assert_eq!(
        who["result"]["structuredContent"]["token"]["scope"],
        "write"
    );
    assert_ne!(create["result"]["isError"], json!(true), "{create}");
    assert!(create["result"]["structuredContent"]["project"]["id"].is_string());
    assert_eq!(
        del["result"]["structuredContent"]["error"]["code"], "forbidden",
        "scope is enforced per call even for unlisted tools"
    );
    assert!(status.success(), "{stderr}");
    assert!(!stderr.contains("yat_"), "no token in logs");
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_reads_the_token_from_a_0600_file_and_rejects_bad_tokens() {
    let (core, cfg, token) = setup(Scope::Read).await;
    let file = core.dir.path().join("token");
    std::fs::write(&file, format!("{token}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let cfg2 = cfg.clone();
    let file2 = file.clone();
    let who = tokio::task::spawn_blocking(move || {
        let mut s = Session::spawn(&cfg2, &[("YANTO_API_TOKEN_FILE", file2.to_str().unwrap())]);
        s.initialize();
        let who = s.request("tools/call", json!({"name":"yanto_whoami","arguments":{}}));
        s.finish();
        who
    })
    .await
    .unwrap();
    assert_eq!(who["result"]["structuredContent"]["token"]["scope"], "read");

    // Missing and invalid tokens end the process before any protocol traffic.
    for env in [
        vec![],
        vec![(
            "YANTO_API_TOKEN",
            "yat_0000000000000000000000_00000000000000000000000000000000",
        )],
    ] {
        let cfg = cfg.clone();
        let (status, stderr) = tokio::task::spawn_blocking(move || {
            let s = Session::spawn(&cfg, &env);
            s.finish()
        })
        .await
        .unwrap();
        assert!(
            !status.success(),
            "must refuse to start without a valid token"
        );
        assert!(stderr.contains("API Token"), "{stderr}");
    }
}
