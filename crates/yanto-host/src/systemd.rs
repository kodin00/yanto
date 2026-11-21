//! [`SystemdPort`] over `systemctl` and `journalctl`, run through a [`CommandPort`] (no shell,
//! own process group, timeout). Unit names are validated before use (04 8.7).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use yanto_core::CoreError;
use yanto_core::ingress::daemon::{parse_show, valid_unit_name};
use yanto_core::ports::{
    ActionResult, CancellationToken, CommandPort, CommandSpec, NullSink, SystemdPort, UnitAction,
    UnitProps,
};

const SHOW_PROPS: &str = "LoadState,ActiveState,SubState,FragmentPath,MainPID,ExecStart";

pub struct HostSystemd {
    runner: Arc<dyn CommandPort>,
}

impl HostSystemd {
    pub fn new(runner: Arc<dyn CommandPort>) -> Self {
        Self { runner }
    }

    fn unit(unit: &str) -> Result<&str, CoreError> {
        if valid_unit_name(unit) {
            Ok(unit)
        } else {
            Err(CoreError::validation("unit", "invalid systemd unit name"))
        }
    }

    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<yanto_core::ports::CommandOutput, CoreError> {
        self.runner
            .run(
                CommandSpec::new(program, args).timeout(timeout).capture(),
                Arc::new(NullSink),
                &CancellationToken::new(),
            )
            .await
    }
}

#[async_trait]
impl SystemdPort for HostSystemd {
    async fn show(&self, unit: &str) -> Result<UnitProps, CoreError> {
        let unit = Self::unit(unit)?;
        let out = self
            .run(
                "systemctl",
                &["show", unit, "-p", SHOW_PROPS, "--no-pager"],
                Duration::from_secs(10),
            )
            .await?;
        Ok(parse_show(&out.stdout))
    }

    async fn control(&self, unit: &str, action: UnitAction) -> Result<ActionResult, CoreError> {
        let unit = Self::unit(unit)?;
        let out = self
            .run(
                "systemctl",
                &[action.as_str(), unit],
                Duration::from_secs(60),
            )
            .await?;
        Ok(ActionResult {
            ok: out.success(),
            output: out.stdout,
        })
    }

    async fn status_text(&self, unit: &str) -> Result<String, CoreError> {
        let unit = Self::unit(unit)?;
        Ok(self
            .run(
                "systemctl",
                &["status", unit, "--no-pager", "-n", "20"],
                Duration::from_secs(10),
            )
            .await?
            .stdout)
    }

    async fn journal_tail(&self, unit: &str, lines: u32) -> Result<String, CoreError> {
        let unit = Self::unit(unit)?;
        let n = lines.min(500).to_string();
        Ok(self
            .run(
                "journalctl",
                &["-u", unit, "-n", &n, "--no-pager"],
                Duration::from_secs(10),
            )
            .await?
            .stdout)
    }

    async fn binary_version(&self, program: &str, args: &[&str]) -> Option<String> {
        let out = self.run(program, args, Duration::from_secs(5)).await.ok()?;
        if !out.success() {
            return None;
        }
        out.stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.chars().take(200).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yanto_core::testing::{FakeCommands, Reply};

    #[tokio::test]
    async fn show_parses_and_arguments_are_a_vector() {
        let fake = FakeCommands::new();
        fake.on(
            "systemctl show frps",
            Reply::stdout("LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=7\nExecStart={ path=/bin/frps ; argv[]=/bin/frps -c /x.toml ; }\n"),
        );
        let sd = HostSystemd::new(fake.clone());
        let p = sd.show("frps").await.unwrap();
        assert_eq!(p.active_state, "active");
        assert_eq!(p.main_pid, Some(7));
        assert!(
            sd.show("frps; rm -rf /").await.is_err(),
            "unit names are validated"
        );
        assert!(sd.control("-x", UnitAction::Stop).await.is_err());
    }
}
