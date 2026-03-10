use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::approvals::NetworkApprovalContext;
use codex_protocol::approvals::NetworkPolicyAmendment;
use codex_protocol::mcp::RequestId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::FileChange;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use serde::Serialize;

use crate::Hook;
use crate::HookApprovalRequest;
use crate::HookEvent;
use crate::HookPayload;
use crate::HookResult;
use crate::command_from_argv;

pub const AGENT_TURN_COMPLETE_EVENT: &str = "agent-turn-complete";
pub const EXEC_APPROVAL_REQUESTED_EVENT: &str = "exec-approval-requested";
pub const APPLY_PATCH_APPROVAL_REQUESTED_EVENT: &str = "apply-patch-approval-requested";
pub const INPUT_REQUESTED_EVENT: &str = "input-requested";
pub const ELICITATION_REQUESTED_EVENT: &str = "elicitation-requested";

pub fn default_notify_events() -> Vec<String> {
    vec![AGENT_TURN_COMPLETE_EVENT.to_string()]
}

fn notify_event_name(hook_event: &HookEvent) -> Option<&'static str> {
    match hook_event {
        HookEvent::AfterAgent { .. } => Some(AGENT_TURN_COMPLETE_EVENT),
        HookEvent::AfterApprovalRequested { event } => match &event.approval {
            HookApprovalRequest::Exec { .. } => Some(EXEC_APPROVAL_REQUESTED_EVENT),
            HookApprovalRequest::ApplyPatch { .. } => Some(APPLY_PATCH_APPROVAL_REQUESTED_EVENT),
        },
        HookEvent::AfterInputRequested { .. } => Some(INPUT_REQUESTED_EVENT),
        HookEvent::AfterElicitationRequested { .. } => Some(ELICITATION_REQUESTED_EVENT),
        HookEvent::AfterToolUse { .. } => None,
    }
}

/// Legacy notify payload appended as the final argv argument for backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,

        /// Messages that the user sent to the agent to initiate the turn.
        input_messages: Vec<String>,

        /// The last message sent by the assistant in the turn.
        last_assistant_message: Option<String>,
    },
    #[serde(rename_all = "kebab-case")]
    ExecApprovalRequested {
        thread_id: String,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        input_messages: Vec<String>,
        last_assistant_message: Option<String>,
        call_id: String,
        reason: Option<String>,
        command: Vec<String>,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        proposed_network_policy_amendments: Option<Vec<NetworkPolicyAmendment>>,
        additional_permissions: Option<PermissionProfile>,
        parsed_cmd: Vec<ParsedCommand>,
        network_approval_context: Option<NetworkApprovalContext>,
    },
    #[serde(rename_all = "kebab-case")]
    ApplyPatchApprovalRequested {
        thread_id: String,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        input_messages: Vec<String>,
        last_assistant_message: Option<String>,
        call_id: String,
        reason: Option<String>,
        changes: HashMap<PathBuf, FileChange>,
        grant_root: Option<PathBuf>,
    },
    #[serde(rename_all = "kebab-case")]
    InputRequested {
        thread_id: String,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        call_id: String,
        input_messages: Vec<String>,
        last_assistant_message: Option<String>,
        questions: Vec<RequestUserInputQuestion>,
    },
    #[serde(rename_all = "kebab-case")]
    ElicitationRequested {
        thread_id: String,
        turn_id: Option<String>,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        server_name: String,
        request_id: RequestId,
        message: String,
    },
}

pub fn legacy_notify_json(payload: &HookPayload) -> Result<String, serde_json::Error> {
    match &payload.hook_event {
        HookEvent::AfterAgent { event } => {
            serde_json::to_string(&UserNotification::AgentTurnComplete {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
            })
        }
        HookEvent::AfterApprovalRequested { event } => match &event.approval {
            HookApprovalRequest::Exec {
                command,
                proposed_execpolicy_amendment,
                proposed_network_policy_amendments,
                additional_permissions,
                parsed_cmd,
                network_approval_context,
            } => serde_json::to_string(&UserNotification::ExecApprovalRequested {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
                call_id: event.call_id.clone(),
                reason: event.reason.clone(),
                command: command.clone(),
                proposed_execpolicy_amendment: proposed_execpolicy_amendment.clone(),
                proposed_network_policy_amendments: proposed_network_policy_amendments.clone(),
                additional_permissions: additional_permissions.clone(),
                parsed_cmd: parsed_cmd.clone(),
                network_approval_context: network_approval_context.clone(),
            }),
            HookApprovalRequest::ApplyPatch {
                changes,
                grant_root,
            } => serde_json::to_string(&UserNotification::ApplyPatchApprovalRequested {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
                call_id: event.call_id.clone(),
                reason: event.reason.clone(),
                changes: changes.clone(),
                grant_root: grant_root.clone(),
            }),
        },
        HookEvent::AfterInputRequested { event } => {
            serde_json::to_string(&UserNotification::InputRequested {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                call_id: event.call_id.clone(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
                questions: event.questions.clone(),
            })
        }
        HookEvent::AfterElicitationRequested { event } => {
            serde_json::to_string(&UserNotification::ElicitationRequested {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                server_name: event.server_name.clone(),
                request_id: event.request_id.clone(),
                message: event.message.clone(),
            })
        }
        HookEvent::AfterToolUse { .. } => Err(serde_json::Error::io(std::io::Error::other(
            "legacy notify payload is not supported for after_tool_use",
        ))),
    }
}

pub(crate) fn notify_hook_with_events(argv: Vec<String>, enabled_events: Vec<String>) -> Hook {
    let argv = Arc::new(argv);
    let enabled_events = Arc::new(enabled_events.into_iter().collect::<HashSet<_>>());
    Hook {
        name: "legacy_notify".to_string(),
        func: Arc::new(move |payload: &HookPayload| {
            let argv = Arc::clone(&argv);
            let enabled_events = Arc::clone(&enabled_events);
            Box::pin(async move {
                let Some(event_name) = notify_event_name(&payload.hook_event) else {
                    return HookResult::Success;
                };
                if !enabled_events.contains(event_name) {
                    return HookResult::Success;
                }

                let mut command = match command_from_argv(&argv) {
                    Some(command) => command,
                    None => return HookResult::Success,
                };
                let Ok(notify_payload) = legacy_notify_json(payload) else {
                    return HookResult::Success;
                };
                command.arg(notify_payload);

                // Backwards-compat: match legacy notify behavior (argv + JSON arg, fire-and-forget).
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());

                match command.spawn() {
                    Ok(_) => HookResult::Success,
                    Err(err) => HookResult::FailedContinue(err.into()),
                }
            })
        }),
    }
}

pub fn notify_hook(argv: Vec<String>) -> Hook {
    notify_hook_with_events(argv, default_notify_events())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use codex_protocol::ThreadId;
    use codex_protocol::approvals::ExecPolicyAmendment;
    use codex_protocol::approvals::NetworkApprovalContext;
    use codex_protocol::approvals::NetworkApprovalProtocol;
    use codex_protocol::approvals::NetworkPolicyAmendment;
    use codex_protocol::approvals::NetworkPolicyRuleAction;
    use codex_protocol::mcp::RequestId;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::parse_command::ParsedCommand;
    use codex_protocol::request_user_input::RequestUserInputQuestion;
    use codex_protocol::request_user_input::RequestUserInputQuestionOption;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;

    use super::*;

    fn expected_notification_json() -> Value {
        json!({
            "type": "agent-turn-complete",
            "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
            "turn-id": "12345",
            "cwd": "/Users/example/project",
            "client": "codex-tui",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds.",
        })
    }

    #[test]
    fn test_user_notification() -> Result<()> {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            client: Some("codex-tui".to_string()),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        };
        let serialized = serde_json::to_string(&notification)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());
        Ok(())
    }

    #[test]
    fn legacy_notify_json_serializes_exec_approval_payload() -> Result<()> {
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            client: Some("codex-tui".to_string()),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterApprovalRequested {
                event: crate::HookEventAfterApprovalRequested {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    turn_id: "12345".to_string(),
                    call_id: "call-1".to_string(),
                    reason: Some("Exec approval required.".to_string()),
                    input_messages: vec!["Run git status".to_string()],
                    last_assistant_message: Some("Checking repository status.".to_string()),
                    approval: HookApprovalRequest::Exec {
                        command: vec!["git".to_string(), "status".to_string()],
                        proposed_execpolicy_amendment: Some(ExecPolicyAmendment::new(vec![
                            "git".to_string(),
                        ])),
                        proposed_network_policy_amendments: Some(vec![NetworkPolicyAmendment {
                            host: "example.com".to_string(),
                            action: NetworkPolicyRuleAction::Allow,
                        }]),
                        additional_permissions: Some(PermissionProfile::default()),
                        parsed_cmd: vec![ParsedCommand::Unknown {
                            cmd: "git status".to_string(),
                        }],
                        network_approval_context: Some(NetworkApprovalContext {
                            host: "example.com".to_string(),
                            protocol: NetworkApprovalProtocol::Https,
                        }),
                    },
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(
            actual,
            json!({
                "type": "exec-approval-requested",
                "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
                "turn-id": "12345",
                "cwd": "/Users/example/project",
                "client": "codex-tui",
                "input-messages": ["Run git status"],
                "last-assistant-message": "Checking repository status.",
                "call-id": "call-1",
                "reason": "Exec approval required.",
                "command": ["git", "status"],
                "proposed-execpolicy-amendment": ["git"],
                "proposed-network-policy-amendments": [{
                    "host": "example.com",
                    "action": "allow"
                }],
                "additional-permissions": {
                    "file_system": null,
                    "macos": null,
                    "network": null
                },
                "parsed-cmd": [{
                    "type": "unknown",
                    "cmd": "git status"
                }],
                "network-approval-context": {
                    "host": "example.com",
                    "protocol": "https"
                }
            })
        );

        Ok(())
    }

    #[test]
    fn legacy_notify_json_serializes_input_requested_payload() -> Result<()> {
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            client: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterInputRequested {
                event: crate::HookEventAfterInputRequested {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    turn_id: "12345".to_string(),
                    call_id: "call-2".to_string(),
                    input_messages: vec!["Need an answer".to_string()],
                    last_assistant_message: Some("Choose one.".to_string()),
                    questions: vec![RequestUserInputQuestion {
                        id: "confirm".to_string(),
                        header: "Confirm".to_string(),
                        question: "Proceed?".to_string(),
                        is_other: false,
                        is_secret: false,
                        options: Some(vec![RequestUserInputQuestionOption {
                            label: "Yes".to_string(),
                            description: "Continue.".to_string(),
                        }]),
                    }],
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual["type"], json!("input-requested"));
        assert_eq!(actual["call-id"], json!("call-2"));
        Ok(())
    }

    #[test]
    fn legacy_notify_json_serializes_elicitation_requested_payload() -> Result<()> {
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            client: Some("codex-app-server".to_string()),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterElicitationRequested {
                event: crate::HookEventAfterElicitationRequested {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    turn_id: Some("12345".to_string()),
                    server_name: "calendar".to_string(),
                    request_id: RequestId::Integer(99),
                    message: "Open the browser".to_string(),
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual["type"], json!("elicitation-requested"));
        assert_eq!(actual["request-id"], json!(99));
        Ok(())
    }

    #[test]
    fn legacy_notify_json_matches_historical_wire_shape() -> Result<()> {
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            client: Some("codex-tui".to_string()),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    turn_id: "12345".to_string(),
                    input_messages: vec![
                        "Rename `foo` to `bar` and update the callsites.".to_string(),
                    ],
                    last_assistant_message: Some(
                        "Rename complete and verified `cargo build` succeeds.".to_string(),
                    ),
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());

        Ok(())
    }
}
