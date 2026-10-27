//! `plannotator-tui rex last|open|terminal`: find the agent in a Rex block and open
//! plannotator-tui in a split to its right, wired to send feedback back to the block. A split is an ordinary pane,
//! so it keeps Rex's frame and theme. `foreground`, `command` and `split` are pure; only `run`
//! touches Rex.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::agent::{AGENT_VAR, BLOCK_VAR, SUBMIT_VAR};
use super::command::Rex;
use crate::herdr::launch::known_host;

/// What the opened pane reviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Review {
    /// The agent's recent replies; `newest` skips the picker.
    Last { newest: bool },
    /// A file or folder; the block's directory when `None`.
    Open(Option<PathBuf>),
    /// The block's last `lines` lines of output.
    Terminal { lines: u32 },
}

/// How feedback reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// As its next message.
    Send,
    /// Into its prompt, for you to edit and submit.
    Paste,
}

/// The program in the foreground of a block, from `com.superlogical.terminal.process` JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Foreground {
    pub(crate) pid: u32,
    pub(crate) cwd: Option<PathBuf>,
    /// The agent's host label when the program is one the hosts crate can read.
    pub(crate) host: Option<&'static str>,
}

/// Read the foreground process. Rex reports the binary's file name, which for Claude Code is
/// its version number, so the name it was invoked by is checked first.
pub(crate) fn foreground(process: &Value) -> Option<Foreground> {
    let fg = process.get("foreground")?;
    let pid = u32::try_from(fg.get("pid")?.as_u64()?).ok()?;
    let host = ["argv0", "invoked_path", "name", "path"]
        .iter()
        .filter_map(|key| fg.get(*key).and_then(Value::as_str))
        .find_map(known_host);
    let cwd = fg.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    Some(Foreground { pid, cwd, host })
}

/// The argv the pane runs: `env` sets the delivery target, then `plannotator-tui rex pane`
/// opens the review.
/// Without an agent in the block, feedback goes to the clipboard.
pub(crate) fn command(
    exe: &Path,
    block: &str,
    fg: &Foreground,
    review: &Review,
    delivery: Delivery,
) -> Result<Vec<String>> {
    let mut argv = vec!["/usr/bin/env".to_owned()];
    if let Some(host) = fg.host {
        argv.push(format!("{BLOCK_VAR}={block}"));
        argv.push(format!("{AGENT_VAR}={host}"));
        if delivery == Delivery::Paste {
            argv.push(format!("{SUBMIT_VAR}=0"));
        }
    }
    argv.extend([exe.display().to_string(), "rex".to_owned(), "pane".to_owned()]);
    match review {
        Review::Last { newest } => {
            let host = fg.host.context("no supported agent runs in this block")?;
            argv.extend(["last".to_owned(), "--pid".to_owned(), fg.pid.to_string()]);
            argv.extend(["--host".to_owned(), host.to_owned()]);
            if *newest {
                argv.push("--newest".to_owned());
            }
        }
        Review::Open(path) => {
            let path = path.clone().or_else(|| fg.cwd.clone()).context("no file or folder to open")?;
            argv.extend(["open".to_owned(), path.display().to_string()]);
        }
        Review::Terminal { lines } => {
            argv.extend(["terminal", "--block", block, "--lines"].map(str::to_owned));
            argv.push(lines.to_string());
        }
    }
    Ok(argv)
}

/// The `session.new_split` arguments for `argv`, to the right of `block`, starting in `cwd`.
pub(crate) fn split(block: &str, cwd: Option<&Path>, argv: &[String]) -> Value {
    let mut options = serde_json::Map::new();
    options.insert("shell".to_owned(), json!("none"));
    options.insert("command".to_owned(), json!(argv));
    if let Some(cwd) = cwd {
        options.insert("cwd".to_owned(), json!(cwd.display().to_string()));
    }
    json!({
        "anchor_id": block,
        "direction": "horizontal",
        "side": "after",
        "focus": true,
        "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "label": "Annotate", "options": options } },
    })
}

/// Open the review in a split to the right of `block`.
pub(crate) fn run(rex: &Rex, block: &str, review: &Review, delivery: Delivery) -> Result<()> {
    let process = rex.terminal_method(block, "process")?;
    let fg = foreground(&process).with_context(|| format!("block {block} has no foreground process"))?;
    let exe = std::env::current_exe().context("finding the plannotator-tui binary")?;
    let argv = command(&exe, block, &fg, review, delivery)?;
    rex.api("session.new_split", &split(block, fg.cwd.as_deref(), &argv))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, reason = "tests assert by panicking")]
mod tests {
    use super::*;

    /// `com.superlogical.terminal.process` for a block running Claude Code, as Rex reports it.
    const CLAUDE: &str = r#"{"last_exit":null,"child":{"pid":59515,"name":"login"},"foreground":{"pid":60223,"name":"2.1.308","path":"/Users/me/.local/share/claude/versions/2.1.308","invoked_path":"/Users/me/.local/bin/claude","argv0":"claude","cwd":"/Users/me/project"}}"#;
    const SHELL: &str = r#"{"foreground":{"pid":700,"name":"fish","argv0":"-fish","cwd":"/Users/me"}}"#;

    fn parse(json: &str) -> Foreground {
        foreground(&serde_json::from_str(json).expect("json")).expect("a foreground process")
    }

    #[test]
    fn claude_code_is_found_by_its_invoked_name_not_its_versioned_binary() {
        let fg = parse(CLAUDE);
        assert_eq!(fg.pid, 60223);
        assert_eq!(fg.host, Some("claude"));
        assert_eq!(fg.cwd.as_deref(), Some(Path::new("/Users/me/project")));
        assert_eq!(parse(SHELL).host, None);
    }

    #[test]
    fn last_reads_the_agents_pid_and_sends_back_to_its_block() {
        let argv = command(
            Path::new("/bin/pt"),
            "block:b",
            &parse(CLAUDE),
            &Review::Last { newest: true },
            Delivery::Send,
        )
        .expect("argv");
        assert_eq!(
            argv,
            [
                "/usr/bin/env",
                "PLANNOTATOR_TUI_REX_BLOCK=block:b",
                "PLANNOTATOR_TUI_DELIVER_AGENT=claude",
                "/bin/pt",
                "rex",
                "pane",
                "last",
                "--pid",
                "60223",
                "--host",
                "claude",
                "--newest"
            ]
        );
        assert!(
            command(
                Path::new("/bin/pt"),
                "block:b",
                &parse(SHELL),
                &Review::Last { newest: false },
                Delivery::Send
            )
            .is_err()
        );
    }

    #[test]
    fn open_without_an_agent_reviews_the_blocks_folder_and_copies_to_the_clipboard() {
        let argv =
            command(Path::new("/bin/pt"), "block:b", &parse(SHELL), &Review::Open(None), Delivery::Send)
                .expect("argv");
        assert_eq!(argv, ["/usr/bin/env", "/bin/pt", "rex", "pane", "open", "/Users/me"]);
    }

    #[test]
    fn pasting_is_passed_on_only_where_there_is_an_agent_to_paste_into() {
        let paste = |json| {
            command(Path::new("/bin/pt"), "block:b", &parse(json), &Review::Open(None), Delivery::Paste)
                .expect("argv")
        };
        assert!(paste(CLAUDE).contains(&"PLANNOTATOR_TUI_REX_SUBMIT=0".to_owned()));
        assert!(!paste(SHELL).iter().any(|arg| arg.starts_with("PLANNOTATOR_TUI_REX")));
    }

    #[test]
    fn a_terminal_review_reads_the_block_from_inside_the_split() {
        let argv = command(
            Path::new("/bin/pt"),
            "block:b",
            &parse(SHELL),
            &Review::Terminal { lines: 80 },
            Delivery::Send,
        )
        .expect("argv");
        assert_eq!(
            argv,
            ["/usr/bin/env", "/bin/pt", "rex", "pane", "terminal", "--block", "block:b", "--lines", "80"]
        );
    }

    #[test]
    fn the_review_opens_to_the_right_of_the_agents_block_and_runs_directly() {
        let split = split("block:b", Some(Path::new("/tmp")), &["/bin/pt".to_owned()]);
        assert_eq!(split["anchor_id"], "block:b");
        assert_eq!((&split["direction"], &split["side"]), (&json!("horizontal"), &json!("after")));
        assert_eq!(split["layout"]["block"]["options"]["shell"], "none");
        assert_eq!(split["layout"]["block"]["options"]["command"], json!(["/bin/pt"]));
        assert_eq!(split["layout"]["block"]["options"]["cwd"], "/tmp");
    }
}
