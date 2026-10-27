//! An agent running in a Rex block, as a delivery target.
//!
//! Sending pastes the feedback into the block as a paste event, so Rex wraps it in the
//! bracketed-paste markers the agent asked for, then presses Enter. In paste mode it leaves
//! Enter to you: the feedback waits in the agent's prompt and the block takes focus, ready to
//! edit. Before writing anything it asks the block's `program_status` whether the agent is at
//! a dialog, the same refusal Herdr gives with `agent_blocked`.

use serde_json::{Value, json};

use super::command::{Rex, RexError, short_id};
use crate::delivery::{Delivery, DeliveryError};

/// `PLANNOTATOR_TUI_REX_BLOCK`: the block to send feedback to, set by `plannotator-tui rex`.
pub(crate) const BLOCK_VAR: &str = "PLANNOTATOR_TUI_REX_BLOCK";
/// `PLANNOTATOR_TUI_DELIVER_AGENT`: the agent's host label, for the footer and the archive.
pub(crate) const AGENT_VAR: &str = "PLANNOTATOR_TUI_DELIVER_AGENT";
/// `PLANNOTATOR_TUI_REX_SUBMIT=0`: paste into the agent's prompt without pressing Enter.
pub(crate) const SUBMIT_VAR: &str = "PLANNOTATOR_TUI_REX_SUBMIT";

#[derive(Debug)]
pub(crate) struct RexAgent {
    rex: Rex,
    block: String,
    agent: Option<String>,
    /// Press Enter after the paste; `false` leaves the feedback in the prompt to edit.
    submit: bool,
}

impl RexAgent {
    /// The target `plannotator-tui rex` named in the environment, or `None` outside such a layer.
    pub(crate) fn from_lookup(env: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |k: &str| env(k).filter(|v| !v.is_empty());
        let block = non_empty(BLOCK_VAR)?;
        let rex = Rex::from_lookup(&env, None).ok()?;
        let submit = env(SUBMIT_VAR).as_deref() != Some("0");
        Some(Self { rex, block, agent: non_empty(AGENT_VAR), submit })
    }
}

impl Delivery for RexAgent {
    fn agent_host(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    fn describe(&self) -> String {
        let block = short_id(&self.block);
        let who = self.agent.as_deref().unwrap_or("agent");
        if self.submit { format!("{who} in {block}") } else { format!("{who} prompt in {block}") }
    }

    fn is_agent(&self) -> bool {
        true
    }

    fn deliver(&self, feedback: &str) -> Result<(), DeliveryError> {
        let status = self.rex.terminal_method(&self.block, "program_status").map_err(unavailable)?;
        if let Some(reason) = blocked_reason(&status) {
            return Err(DeliveryError::Blocked(format!("{} is {reason}", self.describe())));
        }
        let paste = json!({
            "block_id": self.block,
            "event": { "type": "paste", "payload": { "text": feedback } },
        });
        self.rex.api("block.event", &paste).map_err(unavailable)?;
        let next: &[&str] =
            if self.submit { &["send-key", "-b", &self.block, "enter"] } else { &["focus", &self.block] };
        self.rex.run(next, None).map_err(unavailable)?;
        Ok(())
    }
}

fn unavailable(err: RexError) -> DeliveryError {
    match err {
        RexError::NotFound(message) => DeliveryError::Unavailable(message),
        RexError::Failed(err) => DeliveryError::Failed(err),
    }
}

/// What a blocked agent is waiting for, from `program_status` JSON; `None` when it can take a
/// message. A record Rex marks `blocked` is the agent at a permission prompt or a question.
pub(crate) fn blocked_reason(status: &Value) -> Option<String> {
    let records = status.get("records")?.as_array()?;
    let blocked = records.iter().find(|r| r.get("state").and_then(Value::as_str) == Some("blocked"))?;
    Some(match blocked.get("kind").and_then(Value::as_str) {
        Some("permission") => "waiting for a permission answer".to_owned(),
        Some("question") => "waiting for an answer to its question".to_owned(),
        Some("auth") => "waiting for a credential".to_owned(),
        _ => "waiting for you".to_owned(),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests assert by panicking")]
mod tests {
    use super::*;

    fn agent(vars: &[(&str, &str)]) -> Option<RexAgent> {
        let owned: Vec<(String, String)> =
            vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        RexAgent::from_lookup(|k| owned.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone()))
    }

    #[test]
    fn only_a_named_block_inside_a_rex_session_is_a_target() {
        let block = "block:01a17dd5-e47c-754f-b72e-ef739ea1d6db";
        assert!(agent(&[("REX_SESSION", "session:a")]).is_none());
        assert!(agent(&[(BLOCK_VAR, block)]).is_none());
        let target = agent(&[("REX_SESSION", "session:a"), (BLOCK_VAR, block), (AGENT_VAR, "claude")])
            .expect("a target");
        assert_eq!(target.describe(), "claude in a1d6db");
        assert_eq!(target.agent_host(), Some("claude"));
    }

    #[test]
    fn only_an_explicit_zero_turns_sending_into_pasting_and_the_target_says_so() {
        let block = "block:01a17dd5-e47c-754f-b72e-ef739ea1d6db";
        let with = |submit: &'static str| {
            agent(&[("REX_SESSION", "s"), (BLOCK_VAR, block), (AGENT_VAR, "claude"), (SUBMIT_VAR, submit)])
                .expect("a target")
        };
        assert!(with("1").submit && with("").submit);
        assert!(!with("0").submit);
        assert_eq!(with("0").describe(), "claude prompt in a1d6db");
    }

    #[test]
    fn a_working_or_finished_agent_takes_messages_and_a_blocked_one_does_not() {
        let working = json!({"records":[{"state":"working","app":"claude-code"}]});
        assert_eq!(blocked_reason(&working), None);
        assert_eq!(blocked_reason(&json!({"records":[]})), None);
        let asking = json!({"records":[{"state":"done"},{"id":"x","state":"blocked","kind":"permission"}]});
        assert_eq!(blocked_reason(&asking).as_deref(), Some("waiting for a permission answer"));
        let unknown = json!({"records":[{"state":"blocked","kind":"telepathy"}]});
        assert_eq!(blocked_reason(&unknown).as_deref(), Some("waiting for you"));
    }
}
