//! `plannotator-tui rex …`: the launchers a Rex action or an agent runs, and `rex pane`, the
//! entry point of the split they open.

#![allow(clippy::print_stdout, reason = "`--print` is a stdout contract")]

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::command::Rex;
use super::launch::{self, Handoff, Review};
use crate::cli::{USAGE, interactive, last_command, lines_value, wait_on_error};

/// `plannotator-tui rex open|last|terminal|find`: open a review in a split beside a block. The block
/// defaults to `REX_BLOCK`, so an agent running this from its own block reviews itself.
/// `--paste` leaves the feedback in the agent's prompt instead of sending it. `rex pane` is
/// what the split runs.
pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut review = match args.first().map(String::as_str) {
        Some("last") => Review::Last { newest: false },
        Some("open") => Review::Open(None),
        Some("terminal") => Review::Terminal { lines: crate::terminal::DEFAULT_LINES },
        Some("find") => Review::Find { lines: crate::terminal::DEFAULT_LINES },
        Some("pane") => return pane(args.get(1..).unwrap_or_default()),
        _ => anyhow::bail!(USAGE),
    };
    let mut block = None;
    let mut session = None;
    let mut handoff = Handoff::Send;
    let mut print = false;
    let mut rest = args.get(1..).unwrap_or_default().iter();
    while let Some(arg) = rest.next() {
        match (arg.as_str(), &mut review) {
            ("--block", _) => block = Some(rest.next().context("--block needs a value")?.clone()),
            ("--session", _) => session = Some(rest.next().context("--session needs a value")?.clone()),
            ("--paste", _) => handoff = Handoff::Paste,
            ("--newest", Review::Last { newest }) => *newest = true,
            ("--lines", Review::Terminal { lines } | Review::Find { lines }) => {
                *lines = lines_value(rest.next())?;
            }
            ("--print", Review::Terminal { .. }) => print = true,
            (flag, _) if flag.starts_with("--") => anyhow::bail!("unknown flag {flag}\n{USAGE}"),
            (path, Review::Open(open @ None)) => *open = Some(std::path::absolute(path)?),
            (extra, _) => anyhow::bail!("unexpected argument {extra:?}\n{USAGE}"),
        }
    }
    let block = block
        .or_else(|| std::env::var("REX_BLOCK").ok().filter(|b| !b.is_empty()))
        .context("no Rex block: pass --block or run inside Rex (REX_BLOCK is not set)")?;
    let rex = Rex::from_lookup(|k| std::env::var(k).ok(), session)?;
    if let (true, Review::Terminal { lines }) = (print, &review) {
        print!("{}", crate::terminal::document(&super::terminal::read(&rex, &block, *lines)?));
        return Ok(());
    }
    launch::run(&rex, &block, &review, handoff)
}

/// The split's entry point. Rex closes the split when this exits, so an error waits for
/// Enter rather than flash by unread.
fn pane(args: &[String]) -> Result<()> {
    let result = match args.first().map(String::as_str) {
        Some("last") => last_command(args.get(1..).unwrap_or_default()),
        Some("open") => match args.get(1..) {
            Some([path]) => interactive(&PathBuf::from(path)),
            _ => Err(anyhow::anyhow!("rex pane open takes one path\n{USAGE}")),
        },
        Some("terminal") => pane_terminal(args.get(1..).unwrap_or_default()),
        _ => Err(anyhow::anyhow!(USAGE)),
    };
    if let Err(err) = &result {
        wait_on_error(err);
    }
    result
}

fn pane_terminal(args: &[String]) -> Result<()> {
    let mut block = None;
    let mut lines = crate::terminal::DEFAULT_LINES;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--block" => block = Some(rest.next().context("--block needs a value")?.clone()),
            "--lines" => lines = lines_value(rest.next())?,
            other => anyhow::bail!("unexpected argument {other:?}\n{USAGE}"),
        }
    }
    let block = block.context("rex pane terminal needs --block")?;
    let rex = Rex::from_lookup(|k| std::env::var(k).ok(), None)?;
    super::terminal::run(&rex, &block, lines)
}
