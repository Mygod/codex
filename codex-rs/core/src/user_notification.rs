use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::error;
use tracing::warn;

use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::ExecPolicyAmendment;
use codex_protocol::protocol::FileChange;

#[derive(Debug, Default)]
pub(crate) struct UserNotifier {
    notify_command: Option<Vec<String>>,
}

impl UserNotifier {
    pub(crate) fn notify(&self, notification: &UserNotification) {
        if let Some(notify_command) = &self.notify_command
            && !notify_command.is_empty()
        {
            self.invoke_notify(notify_command, notification)
        }
    }

    fn invoke_notify(&self, notify_command: &[String], notification: &UserNotification) {
        let Ok(json) = serde_json::to_string(&notification) else {
            error!("failed to serialise notification payload");
            return;
        };

        let mut command = std::process::Command::new(&notify_command[0]);
        if notify_command.len() > 1 {
            command.args(&notify_command[1..]);
        }
        command.arg(json);

        // Fire-and-forget – we do not wait for completion.
        if let Err(e) = command.spawn() {
            warn!("failed to spawn notifier '{}': {e}", notify_command[0]);
        }
    }

    pub(crate) fn new(notify: Option<Vec<String>>) -> Self {
        Self {
            notify_command: notify,
        }
    }
}

/// User can configure a program that will receive notifications. Each
/// notification is serialized as JSON and passed as an argument to the
/// program.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        turn_id: String,
        cwd: String,

        /// Messages that the user sent to the agent to initiate the turn.
        input_messages: Vec<String>,

        /// The last message sent by the assistant in the turn.
        last_assistant_message: Option<String>,
    },
    #[serde(rename_all = "kebab-case")]
    ApprovalRequested {
        #[serde(flatten)]
        approval: ApprovalNotification,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ApprovalContext {
    pub(crate) thread_id: String,
    pub(crate) turn_id: String,
    pub(crate) cwd: String,
    pub(crate) input_messages: Vec<String>,
    pub(crate) last_assistant_message: Option<String>,
    pub(crate) call_id: String,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "approval-kind", rename_all = "kebab-case")]
pub(crate) enum ApprovalNotification {
    #[serde(rename_all = "kebab-case")]
    Exec {
        #[serde(flatten)]
        context: ApprovalContext,
        command: Vec<String>,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        parsed_cmd: Vec<ParsedCommand>,
    },
    #[serde(rename_all = "kebab-case")]
    ApplyPatch {
        #[serde(flatten)]
        context: ApprovalContext,
        changes: HashMap<PathBuf, FileChange>,
        grant_root: Option<PathBuf>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn test_user_notification() -> Result<()> {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        };
        let serialized = serde_json::to_string(&notification)?;
        assert_eq!(
            serialized,
            r#"{"type":"agent-turn-complete","thread-id":"b5f6c1c2-1111-2222-3333-444455556666","turn-id":"12345","cwd":"/Users/example/project","input-messages":["Rename `foo` to `bar` and update the callsites."],"last-assistant-message":"Rename complete and verified `cargo build` succeeds."}"#
        );
        Ok(())
    }

    #[test]
    fn test_user_notification_exec_approval() -> Result<()> {
        let context = ApprovalContext {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            input_messages: vec!["Run git status".to_string()],
            last_assistant_message: Some("Checking repository status.".to_string()),
            call_id: "call-1".to_string(),
            reason: Some("Exec approval required.".to_string()),
        };
        let notification = UserNotification::ApprovalRequested {
            approval: ApprovalNotification::Exec {
                context,
                command: vec!["git".to_string(), "status".to_string()],
                proposed_execpolicy_amendment: Some(ExecPolicyAmendment::new(vec![
                    "git".to_string(),
                ])),
                parsed_cmd: vec![ParsedCommand::Unknown {
                    cmd: "git status".to_string(),
                }],
            },
        };
        let serialized = serde_json::to_value(&notification)?;
        assert_eq!(
            serialized,
            json!({
                "type": "approval-requested",
                "approval-kind": "exec",
                "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
                "turn-id": "12345",
                "cwd": "/Users/example/project",
                "input-messages": ["Run git status"],
                "last-assistant-message": "Checking repository status.",
                "call-id": "call-1",
                "reason": "Exec approval required.",
                "command": ["git", "status"],
                "proposed-execpolicy-amendment": ["git"],
                "parsed-cmd": [
                    {"type": "unknown", "cmd": "git status"}
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn test_user_notification_apply_patch_approval() -> Result<()> {
        let context = ApprovalContext {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            input_messages: vec!["Apply patch to main.rs".to_string()],
            last_assistant_message: None,
            call_id: "call-2".to_string(),
            reason: None,
        };
        let mut changes = HashMap::new();
        changes.insert(
            PathBuf::from("src/main.rs"),
            FileChange::Update {
                unified_diff: "@@ -1 +1 @@\n-old\n+new\n".to_string(),
                move_path: None,
            },
        );
        let notification = UserNotification::ApprovalRequested {
            approval: ApprovalNotification::ApplyPatch {
                context,
                changes,
                grant_root: Some(PathBuf::from("/Users/example/project")),
            },
        };
        let serialized = serde_json::to_value(&notification)?;
        assert_eq!(
            serialized,
            json!({
                "type": "approval-requested",
                "approval-kind": "apply-patch",
                "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
                "turn-id": "12345",
                "cwd": "/Users/example/project",
                "input-messages": ["Apply patch to main.rs"],
                "last-assistant-message": null,
                "call-id": "call-2",
                "reason": null,
                "changes": {
                    "src/main.rs": {
                        "type": "update",
                        "unified_diff": "@@ -1 +1 @@\n-old\n+new\n",
                        "move_path": null
                    }
                },
                "grant-root": "/Users/example/project"
            })
        );
        Ok(())
    }
}
