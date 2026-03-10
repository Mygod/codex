#![cfg(not(target_os = "windows"))]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use codex_core::config::Constrained;
use codex_core::config::types::NotifyEvent;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::ReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::request_user_input::RequestUserInputAnswer;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use core_test_support::fs_wait;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::Instant;
use tokio::time::sleep;

use responses::ev_assistant_message;
use responses::ev_completed;
use responses::ev_function_call;
use responses::ev_response_created;
use responses::sse;
use responses::start_mock_server;

struct NotifyRecorder {
    _dir: TempDir,
    payloads_path: PathBuf,
    script_path: String,
}

impl NotifyRecorder {
    fn new() -> anyhow::Result<Self> {
        let dir = TempDir::new()?;
        let notify_script = dir.path().join("notify.sh");
        std::fs::write(
            &notify_script,
            r#"#!/bin/bash
set -euo pipefail
payload_path="$(dirname "${0}")/notify.jsonl"
printf '%s\n' "${@: -1}" >> "${payload_path}""#,
        )?;
        std::fs::set_permissions(&notify_script, std::fs::Permissions::from_mode(0o755))?;

        Ok(Self {
            payloads_path: dir.path().join("notify.jsonl"),
            script_path: notify_script.to_string_lossy().into_owned(),
            _dir: dir,
        })
    }
}

fn parse_payloads(raw: &str) -> anyhow::Result<Vec<Value>> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn wait_for_payloads(path: &Path, expected_count: usize) -> anyhow::Result<Vec<Value>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(raw) = tokio::fs::read_to_string(path).await {
            let payloads = parse_payloads(&raw)?;
            if payloads.len() >= expected_count {
                return Ok(payloads);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for {expected_count} notify payload(s)");
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn submit_turn(
    test: &TestCodex,
    prompt: &str,
    approval_policy: AskForApproval,
    sandbox_policy: SandboxPolicy,
    collaboration_mode: Option<CollaborationMode>,
) -> anyhow::Result<()> {
    let session_model = test.session_configured.model.clone();
    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd.path().to_path_buf(),
            approval_policy,
            sandbox_policy,
            model: session_model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode,
            personality: None,
        })
        .await?;
    Ok(())
}

fn shell_command_event(call_id: &str, command: &str) -> anyhow::Result<Value> {
    let args = json!({
        "command": command,
        "timeout_ms": 1_000_u64,
    });
    let args_str = serde_json::to_string(&args)?;
    Ok(ev_function_call(call_id, "shell_command", &args_str))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_turn_complete_emits_legacy_payload() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let sse1 = sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]);

    responses::mount_sse_once(&server, sse1).await;

    let recorder = NotifyRecorder::new()?;

    let TestCodex { codex, .. } = test_codex()
        .with_config(move |cfg| cfg.notify = Some(vec![recorder.script_path]))
        .build(&server)
        .await?;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello world".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    fs_wait::wait_for_path_exists(&recorder.payloads_path, Duration::from_secs(5)).await?;
    let payloads = wait_for_payloads(&recorder.payloads_path, 1).await?;
    let payload = &payloads[0];

    assert_eq!(payload["type"], json!("agent-turn-complete"));
    assert_eq!(payload["input-messages"], json!(["hello world"]));
    assert_eq!(payload["last-assistant-message"], json!("Done"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn notify_exec_approval_requested_is_opt_in() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let approval_policy = AskForApproval::OnRequest;
    let sandbox_policy = SandboxPolicy::new_read_only_policy();
    let sandbox_policy_for_config = sandbox_policy.clone();
    let recorder = NotifyRecorder::new()?;
    let script_path = recorder.script_path.clone();

    let mut builder = test_codex().with_config(move |cfg| {
        cfg.notify = Some(vec![script_path]);
        cfg.notify_events = vec![NotifyEvent::ExecApprovalRequested];
        cfg.permissions.approval_policy = Constrained::allow_any(approval_policy);
        cfg.permissions.sandbox_policy = Constrained::allow_any(sandbox_policy_for_config);
    });
    let test = builder.build(&server).await?;

    let call_id = "notify-exec-approval";
    let command = "touch notify-approval.txt";
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            shell_command_event(call_id, command)?,
            ev_completed("resp-1"),
        ]),
    )
    .await;
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(
        &test,
        "please create the file",
        approval_policy,
        sandbox_policy.clone(),
        None,
    )
    .await?;

    let approval = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ExecApprovalRequest(event) => Some(event.clone()),
        _ => None,
    })
    .await;
    let payloads = wait_for_payloads(&recorder.payloads_path, 1).await?;
    let payload = &payloads[0];

    assert_eq!(payload["type"], json!("exec-approval-requested"));
    assert_eq!(payload["call-id"], json!(call_id));
    assert_eq!(payload["input-messages"], json!(["please create the file"]));
    assert_eq!(
        payload["command"]
            .as_array()
            .and_then(|command| command.last())
            .cloned(),
        Some(json!(command))
    );

    test.codex
        .submit(Op::ExecApproval {
            id: approval.effective_approval_id(),
            turn_id: None,
            decision: ReviewDecision::Approved,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_request_user_input_is_opt_in() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let recorder = NotifyRecorder::new()?;
    let script_path = recorder.script_path.clone();

    let mut builder = test_codex().with_config(move |cfg| {
        cfg.notify = Some(vec![script_path]);
        cfg.notify_events = vec![NotifyEvent::InputRequested];
    });
    let test = builder.build(&server).await?;

    let call_id = "notify-request-user-input";
    let request_args = json!({
        "questions": [{
            "id": "confirm",
            "header": "Confirm",
            "question": "Proceed?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue."
            }, {
                "label": "No",
                "description": "Stop."
            }]
        }]
    })
    .to_string();

    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call(call_id, "request_user_input", &request_args),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let results = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "thanks"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    let collaboration_mode = CollaborationMode {
        mode: ModeKind::Plan,
        settings: Settings {
            model: test.session_configured.model.clone(),
            reasoning_effort: None,
            developer_instructions: None,
        },
    };
    submit_turn(
        &test,
        "please confirm",
        AskForApproval::Never,
        SandboxPolicy::DangerFullAccess,
        Some(collaboration_mode),
    )
    .await?;

    let request = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::RequestUserInput(request) => Some(request.clone()),
        _ => None,
    })
    .await;
    let payloads = wait_for_payloads(&recorder.payloads_path, 1).await?;
    let payload = &payloads[0];

    assert_eq!(payload["type"], json!("input-requested"));
    assert_eq!(payload["call-id"], json!(call_id));
    assert_eq!(payload["input-messages"], json!(["please confirm"]));
    assert_eq!(payload["questions"][0]["id"], json!("confirm"));

    let mut answers = HashMap::new();
    answers.insert(
        "confirm".to_string(),
        RequestUserInputAnswer {
            answers: vec!["yes".to_string()],
        },
    );
    test.codex
        .submit(Op::UserInputAnswer {
            id: request.turn_id.clone(),
            response: RequestUserInputResponse { answers },
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let req = results.single_request();
    let output = req.function_call_output(call_id);
    assert_eq!(output["call_id"], json!(call_id));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_emits_exactly_one_top_level_turn_complete_notification() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let recorder = NotifyRecorder::new()?;
    let script_path = recorder.script_path.clone();
    let review_json = json!({
        "findings": [],
        "overall_correctness": "good",
        "overall_explanation": "Review finished.",
        "overall_confidence_score": 0.8
    })
    .to_string();

    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", &review_json),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_codex().with_config(move |cfg| {
        cfg.notify = Some(vec![script_path]);
        cfg.notify_events = vec![NotifyEvent::AgentTurnComplete];
    });
    let test = builder.build(&server).await?;

    test.codex
        .submit(Op::Review {
            review_request: ReviewRequest {
                target: ReviewTarget::Custom {
                    instructions: "please review".to_string(),
                },
                user_facing_hint: None,
            },
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    sleep(Duration::from_millis(100)).await;
    let payloads = wait_for_payloads(&recorder.payloads_path, 1).await?;

    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["type"], json!("agent-turn-complete"));

    Ok(())
}
