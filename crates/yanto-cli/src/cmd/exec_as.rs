//! Hidden helper `yanto exec-as`: drops privileges and execs a program. `cron-run` starts the
//! entry's command through it because the workspace forbids `unsafe` (no `pre_exec` hook).
//! It only ever lowers privileges: a non-root caller cannot use it to gain any.

use clap::Args;

#[derive(Debug, Clone, Args)]
pub struct ExecAsArgs {
    #[arg(long)]
    pub uid: u32,
    #[arg(long)]
    pub gid: u32,
    /// User name, for `initgroups`
    #[arg(long)]
    pub name: String,
    /// Program and arguments, after `--`
    #[arg(last = true, required = true)]
    pub argv: Vec<String>,
}

pub fn run(args: &ExecAsArgs) -> anyhow::Result<()> {
    let (program, rest) = args.argv.split_first().expect("clap requires one");
    let err = yanto_host::exec_as(&args.name, args.uid, args.gid, program, rest);
    eprintln!("yanto exec-as: {err}");
    std::process::exit(126)
}
