//! Prompt seam: `inquire` in production, scripted answers in tests.

use anyhow::Context;

pub trait Prompter {
    fn text(&mut self, msg: &str, default: &str) -> anyhow::Result<String>;
    fn confirm(&mut self, msg: &str, default: bool) -> anyhow::Result<bool>;
    /// Returns the selected option indices.
    fn multiselect(
        &mut self,
        msg: &str,
        options: &[&str],
        defaults: &[usize],
    ) -> anyhow::Result<Vec<usize>>;
    fn select(&mut self, msg: &str, options: &[&str]) -> anyhow::Result<usize>;
}

/// Ask for text until `validate` accepts it, echoing the reason on rejection.
pub fn ask_validated<T>(
    p: &mut dyn Prompter,
    msg: &str,
    default: &str,
    validate: impl Fn(&str) -> Result<T, String>,
) -> anyhow::Result<T> {
    loop {
        let answer = p.text(msg, default)?;
        match validate(answer.trim()) {
            Ok(v) => return Ok(v),
            Err(why) => eprintln!("  invalid: {why}"),
        }
    }
}

pub struct InquirePrompter;

fn map(e: inquire::InquireError) -> anyhow::Error {
    match e {
        inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted => {
            anyhow::anyhow!("aborted by user")
        }
        other => anyhow::Error::new(other)
            .context("interactive prompt failed (no terminal? pass --non-interactive and flags)"),
    }
}

impl Prompter for InquirePrompter {
    fn text(&mut self, msg: &str, default: &str) -> anyhow::Result<String> {
        let mut t = inquire::Text::new(msg);
        if !default.is_empty() {
            t = t.with_default(default);
        }
        t.prompt().map_err(map)
    }

    fn confirm(&mut self, msg: &str, default: bool) -> anyhow::Result<bool> {
        inquire::Confirm::new(msg)
            .with_default(default)
            .prompt()
            .map_err(map)
    }

    fn multiselect(
        &mut self,
        msg: &str,
        options: &[&str],
        defaults: &[usize],
    ) -> anyhow::Result<Vec<usize>> {
        let opts: Vec<String> = options.iter().map(|s| (*s).to_string()).collect();
        let chosen = inquire::MultiSelect::new(msg, opts.clone())
            .with_default(defaults)
            .raw_prompt()
            .map_err(map)
            .context("multi-select")?;
        Ok(chosen.into_iter().map(|o| o.index).collect())
    }

    fn select(&mut self, msg: &str, options: &[&str]) -> anyhow::Result<usize> {
        let opts: Vec<String> = options.iter().map(|s| (*s).to_string()).collect();
        let c = inquire::Select::new(msg, opts).raw_prompt().map_err(map)?;
        Ok(c.index)
    }
}

#[cfg(test)]
pub use scripted::Scripted;

#[cfg(test)]
mod scripted {
    use super::*;
    use std::collections::VecDeque;

    /// Pops pre-recorded answers in order; panics if the wizard asks more
    /// questions than scripted (a test of "no prompt when flag given").
    #[derive(Default)]
    pub struct Scripted {
        pub texts: VecDeque<String>,
        pub confirms: VecDeque<bool>,
        pub multis: VecDeque<Vec<usize>>,
        pub selects: VecDeque<usize>,
        pub asked: Vec<String>,
    }

    impl Prompter for Scripted {
        fn text(&mut self, msg: &str, _default: &str) -> anyhow::Result<String> {
            self.asked.push(msg.into());
            Ok(self.texts.pop_front().expect("unscripted text prompt"))
        }
        fn confirm(&mut self, msg: &str, _d: bool) -> anyhow::Result<bool> {
            self.asked.push(msg.into());
            Ok(self.confirms.pop_front().expect("unscripted confirm"))
        }
        fn multiselect(
            &mut self,
            msg: &str,
            _o: &[&str],
            _d: &[usize],
        ) -> anyhow::Result<Vec<usize>> {
            self.asked.push(msg.into());
            Ok(self.multis.pop_front().expect("unscripted multiselect"))
        }
        fn select(&mut self, msg: &str, _o: &[&str]) -> anyhow::Result<usize> {
            self.asked.push(msg.into());
            Ok(self.selects.pop_front().expect("unscripted select"))
        }
    }
}
