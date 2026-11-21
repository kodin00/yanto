//! `containers_list`, `container_logs`, `container_start|stop|restart`. No exec (D24).

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::ports::LogStream;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_containers_list",
        "containers_list",
        "List Docker containers visible to you, with their Project and state.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.containers.list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_container_logs",
            "container_logs",
            "Last log lines of a container (default 200).",
            Scope::Read,
            Class::Read,
            |s, a, args: LogsArgs| async move {
                let lines = s.containers.logs(&a, &args.id, args.tail).await?;
                let text: String = lines
                    .iter()
                    .map(|l| {
                        let stream = match l.stream {
                            LogStream::Stdout => "stdout",
                            LogStream::Stderr => "stderr",
                        };
                        format!(
                            "{} [{stream}] {}\n",
                            l.timestamp.as_deref().unwrap_or("-"),
                            l.text
                        )
                    })
                    .collect();
                Ok(serde_json::json!({ "logs": text }))
            },
        )
        .untrusted(),
    );
    set.add(ToolDef::new(
        "yanto_container_start",
        "container_start",
        "Start a stopped container.",
        Scope::Write,
        Class::Write,
        |s, a, args: IdArgs| async move {
            s.containers.start(&a, &args.id).await?;
            done()
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_container_stop",
            "container_stop",
            "Stop a running container.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.containers.stop(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_container_restart",
            "container_restart",
            "Restart a container.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.containers.restart(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    /// Container id or name.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogsArgs {
    /// Container id or name.
    id: String,
    /// Number of trailing lines.
    tail: Option<u32>,
}
