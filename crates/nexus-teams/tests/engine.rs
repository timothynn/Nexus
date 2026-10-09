//! End-to-end mission engine behavior.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use nexus_permissions::PermissionDecision;
use nexus_teams::{
    BotAction, BotBrain, BotProfile, BotTurn, BrainRouter, Budget, ControlInput, FloorPolicyKind,
    Goal, MemoryEventSink, MessageKind, MissionEngine, MissionState, MissionStatus, ScriptedBrain,
    SimulatedBrain, TaskId, TaskStatus, TeamError, TeamEvent, TeamSpec, TurnContext,
};
use tokio_util::sync::CancellationToken;

fn bot(handle: &str, archetype: &str) -> BotProfile {
    BotProfile::from_archetype(handle, archetype).expect("archetype")
}

fn squad() -> TeamSpec {
    TeamSpec::new(
        "squad",
        vec![
            bot("lead", "lead"),
            bot("ada", "engineer"),
            bot("rev", "reviewer"),
        ],
    )
    .with_approvers(["rev"])
}

fn simulated() -> BrainRouter {
    BrainRouter::new(Arc::new(SimulatedBrain))
}

fn scripted(brain: ScriptedBrain) -> BrainRouter {
    BrainRouter::new(Arc::new(brain))
}

async fn run(team: TeamSpec, brains: BrainRouter) -> (nexus_teams::MissionReport, Vec<TeamEvent>) {
    let sink = Arc::new(MemoryEventSink::default());
    let report = MissionEngine::new(team, Goal::new("Add a --json flag to the CLI"), brains)
        .expect("engine")
        .with_sink(sink.clone())
        .run()
        .await
        .expect("mission runs");
    (report, sink.events())
}

fn replay(events: &[TeamEvent]) -> MissionState {
    MissionState::replay(events).expect("replayable")
}

#[tokio::test]
async fn simulated_team_completes_with_reviewer_approval() {
    let (report, events) = run(squad(), simulated()).await;
    assert_eq!(report.status, MissionStatus::Completed, "{report:?}");
    assert_eq!(report.tasks_total, report.tasks_done);
    assert!(report.tasks_total >= 2);
    assert!(report.summary.is_some());
    assert!(events.iter().any(
        |event| matches!(event, TeamEvent::VoteCast { by, approve: true, .. } if by == "rev")
    ));
    assert!(matches!(
        events.last(),
        Some(TeamEvent::MissionFinished { .. })
    ));
    assert!(report.usage.total() > 0);
}

#[tokio::test]
async fn replaying_events_reproduces_final_state() {
    let (report, events) = run(squad(), simulated()).await;
    let state = replay(&events);
    assert_eq!(state.status, report.status);
    assert_eq!(state.message_count(), report.messages);
    assert_eq!(state.build_report().members, report.members);
    assert_eq!(state.report.as_ref(), Some(&report));
}

#[tokio::test]
async fn every_floor_policy_terminates() {
    for policy in FloorPolicyKind::ALL {
        let (report, _) = run(squad().with_policy(policy), simulated()).await;
        assert!(report.status.is_terminal(), "{policy}: {report:?}");
        assert!(
            matches!(
                report.status,
                MissionStatus::Completed | MissionStatus::OutOfBudget | MissionStatus::Stalled
            ),
            "{policy}: {report:?}"
        );
    }
}

#[tokio::test]
async fn lead_directed_and_mention_driven_complete() {
    for policy in [
        FloorPolicyKind::LeadDirected,
        FloorPolicyKind::MentionDriven,
        FloorPolicyKind::Broadcast,
    ] {
        let (report, _) = run(squad().with_policy(policy), simulated()).await;
        assert_eq!(
            report.status,
            MissionStatus::Completed,
            "{policy}: {report:?}"
        );
    }
}

#[tokio::test]
async fn non_leads_cannot_propose_completion() {
    let brain = ScriptedBrain::new()
        .with_turn("lead", vec![BotAction::post("@ada go")])
        .with_turn(
            "ada",
            vec![BotAction::ProposeCompletion {
                summary: "I declare victory".to_owned(),
            }],
        );
    let team = squad()
        .with_policy(FloorPolicyKind::MentionDriven)
        .with_budget(Budget {
            max_rounds: 3,
            ..Budget::default()
        });
    let (report, events) = run(team, scripted(brain)).await;
    assert_ne!(report.status, MissionStatus::Completed);
    assert!(events.iter().any(|event| matches!(
        event,
        TeamEvent::ActionDenied { bot, action, .. } if bot == "ada" && action == "propose_completion"
    )));
    assert_eq!(report.members["ada"].denied, 1);
}

#[tokio::test]
async fn profile_permissions_can_grant_and_revoke() {
    let mut lead = bot("lead", "lead");
    lead.permissions
        .insert("team.task.*".to_owned(), PermissionDecision::Deny);
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::CreateTask {
            title: "x".to_owned(),
            description: String::new(),
            assignee: None,
            depends_on: Vec::new(),
        }],
    );
    let team = TeamSpec::new("t", vec![lead, bot("ada", "engineer")]).with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let (report, events) = run(team, scripted(brain)).await;
    assert_eq!(report.tasks_total, 0);
    assert!(events.iter().any(
        |event| matches!(event, TeamEvent::ActionDenied { action, .. } if action == "create_task")
    ));
}

#[tokio::test]
async fn reviewer_rejection_returns_mission_to_active() {
    let brain = ScriptedBrain::new()
        .with_turn(
            "lead",
            vec![BotAction::ProposeCompletion {
                summary: "done?".to_owned(),
            }],
        )
        .with_turn(
            "rev",
            vec![BotAction::Vote {
                approve: false,
                reason: "no tests".to_owned(),
            }],
        );
    let team = squad().with_budget(Budget {
        max_rounds: 3,
        stall_rounds: 0,
        ..Budget::default()
    });
    let (report, events) = run(team, scripted(brain)).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TeamEvent::ProposalResolved { accepted: false }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TeamEvent::StatusChanged { status: MissionStatus::Active, reason } if reason.contains("no tests")
    )));
    assert_eq!(report.status, MissionStatus::OutOfBudget);
}

#[tokio::test]
async fn lead_decides_when_there_are_no_approvers() {
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::ProposeCompletion {
            summary: "shipped".to_owned(),
        }],
    );
    let team = TeamSpec::new("t", vec![bot("lead", "lead"), bot("ada", "engineer")]);
    let (report, _) = run(team, scripted(brain)).await;
    assert_eq!(report.status, MissionStatus::Completed);
    assert_eq!(report.summary.as_deref(), Some("shipped"));
}

#[tokio::test]
async fn idle_teams_stall() {
    let team = squad().with_budget(Budget {
        stall_rounds: 2,
        ..Budget::default()
    });
    let (report, events) = run(team, scripted(ScriptedBrain::new())).await;
    assert_eq!(report.status, MissionStatus::Stalled);
    assert_eq!(report.rounds, 2);
    assert!(events.iter().any(|event| matches!(
        event,
        TeamEvent::MessagePosted { message } if message.author == "system" && message.mentions == vec!["lead".to_owned()]
    )));
}

#[tokio::test]
async fn turn_budget_is_enforced() {
    let team = squad()
        .with_policy(FloorPolicyKind::Broadcast)
        .with_budget(Budget {
            max_turns: 4,
            stall_rounds: 0,
            ..Budget::default()
        });
    let (report, _) = run(team, scripted(ScriptedBrain::new())).await;
    assert_eq!(report.status, MissionStatus::OutOfBudget);
    assert_eq!(report.turns, 4);
}

#[tokio::test]
async fn token_budget_is_enforced() {
    let team = squad().with_budget(Budget {
        max_tokens: Some(1),
        ..Budget::default()
    });
    let (report, _) = run(team, simulated()).await;
    assert_eq!(report.status, MissionStatus::OutOfBudget);
    assert!(report.reason.contains("token"));
}

#[tokio::test]
async fn cancellation_stops_the_mission() {
    let engine = MissionEngine::new(squad(), Goal::new("anything"), simulated()).expect("engine");
    engine.control().cancel();
    let report = engine.run().await.expect("report");
    assert_eq!(report.status, MissionStatus::Cancelled);
    assert_eq!(report.turns, 0);
}

#[tokio::test]
async fn human_approval_gates_completion() {
    let team = TeamSpec::new("t", vec![bot("lead", "lead"), bot("ada", "engineer")])
        .with_approvers(["human"]);
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::ProposeCompletion {
            summary: "ready".to_owned(),
        }],
    );

    let not_interactive =
        MissionEngine::new(team.clone(), Goal::new("g"), scripted(ScriptedBrain::new()))
            .expect("engine")
            .run()
            .await;
    assert!(matches!(not_interactive, Err(TeamError::InvalidTeam(_))));

    let engine = MissionEngine::new(team, Goal::new("g"), scripted(brain))
        .expect("engine")
        .interactive(true);
    let control = engine.control();
    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(50)).await;
    control.approve().expect("send");
    let report = handle.await.expect("join").expect("report");
    assert_eq!(report.status, MissionStatus::Completed);
    assert!(report.reason.contains("@human"));
}

#[tokio::test]
async fn human_rejection_sends_team_back_to_work() {
    let team = TeamSpec::new("t", vec![bot("lead", "lead")])
        .with_approvers(["human"])
        .with_budget(Budget {
            max_rounds: 2,
            stall_rounds: 0,
            ..Budget::default()
        });
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::ProposeCompletion {
            summary: "ready".to_owned(),
        }],
    );
    let engine = MissionEngine::new(team, Goal::new("g"), scripted(brain))
        .expect("engine")
        .interactive(true);
    let control = engine.control();
    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(50)).await;
    control.reject("needs docs").expect("send");
    let report = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("rejection is handled promptly")
        .expect("join")
        .expect("report");
    assert_eq!(report.status, MissionStatus::OutOfBudget);
}

#[tokio::test]
async fn questions_without_a_human_proceed() {
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::AskHuman {
            question: "Which DB?".to_owned(),
        }],
    );
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let (_, events) = run(team, scripted(brain)).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TeamEvent::HumanAnswered { answer: None }))
    );
}

#[tokio::test]
async fn interactive_questions_wait_for_answers() {
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::AskHuman {
            question: "Which DB?".to_owned(),
        }],
    );
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let sink = Arc::new(MemoryEventSink::default());
    let engine = MissionEngine::new(team, Goal::new("g"), scripted(brain))
        .expect("engine")
        .interactive(true)
        .with_sink(sink.clone());
    let control = engine.control();
    let handle = tokio::spawn(engine.run());
    tokio::time::sleep(Duration::from_millis(50)).await;
    control.answer("Postgres").expect("send");
    handle.await.expect("join").expect("report");
    let state = replay(&sink.events());
    assert!(state.messages.iter().any(|m| m.author == "human"
        && m.text.contains("Postgres")
        && m.mentions == vec!["lead".to_owned()]));
}

#[tokio::test]
async fn human_posts_bring_mentioned_bots_in() {
    let team = squad()
        .with_policy(FloorPolicyKind::MentionDriven)
        .with_budget(Budget {
            max_rounds: 1,
            ..Budget::default()
        });
    let sink = Arc::new(MemoryEventSink::default());
    let engine = MissionEngine::new(team, Goal::new("g"), scripted(ScriptedBrain::new()))
        .expect("engine")
        .with_sink(sink.clone());
    engine
        .control()
        .post("general", "@rev take a look please")
        .expect("queued");
    engine.run().await.expect("report");
    let events = sink.events();
    assert!(events.iter().any(|event| matches!(
        event,
        TeamEvent::TurnStarted { bot, reason, .. } if bot == "rev" && reason.contains("@human")
    )));
}

#[tokio::test]
async fn pause_and_resume_are_recorded() {
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let sink = Arc::new(MemoryEventSink::default());
    let engine = MissionEngine::new(team, Goal::new("g"), simulated())
        .expect("engine")
        .with_sink(sink.clone());
    let control = engine.control();
    control.send(ControlInput::Pause).expect("pause");
    control.send(ControlInput::Resume).expect("resume");
    engine.run().await.expect("report");
    let statuses = sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            TeamEvent::StatusChanged { status, .. } => Some(status),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        &statuses[..2],
        &[MissionStatus::Paused, MissionStatus::Active]
    );
}

#[tokio::test]
async fn direct_messages_are_private() {
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![BotAction::DirectMessage {
            to: "ada".to_owned(),
            text: "quiet word".to_owned(),
        }],
    );
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let (_, events) = run(team, scripted(brain)).await;
    let state = replay(&events);
    let dm = state.channel("dm:ada+lead").expect("dm channel");
    assert!(dm.direct);
    assert_eq!(
        state
            .visible_to("ada")
            .filter(|m| m.channel == dm.name)
            .count(),
        1
    );
    assert_eq!(
        state
            .visible_to("rev")
            .filter(|m| m.channel == dm.name)
            .count(),
        0
    );
    let message = state
        .messages
        .iter()
        .find(|m| m.channel == dm.name)
        .expect("dm");
    assert_eq!(message.kind, MessageKind::Direct);
}

#[tokio::test]
async fn dependencies_gate_task_progress() {
    let brain = ScriptedBrain::new().with_turn(
        "lead",
        vec![
            BotAction::CreateTask {
                title: "design".to_owned(),
                description: String::new(),
                assignee: Some("ada".to_owned()),
                depends_on: Vec::new(),
            },
            BotAction::CreateTask {
                title: "build".to_owned(),
                description: String::new(),
                assignee: Some("ada".to_owned()),
                depends_on: vec![TaskId(1)],
            },
            BotAction::UpdateTask {
                task: TaskId(2),
                status: TaskStatus::InProgress,
                note: None,
            },
            BotAction::CreateTask {
                title: "loop".to_owned(),
                description: String::new(),
                assignee: None,
                depends_on: vec![TaskId(9)],
            },
        ],
    );
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let (_, events) = run(team, scripted(brain)).await;
    let state = replay(&events);
    assert_eq!(state.board.len(), 2);
    assert_eq!(
        state.board.get(TaskId(2)).expect("task").status,
        TaskStatus::Todo
    );
    assert_eq!(state.stats["lead"].denied, 2);
}

#[tokio::test]
async fn channels_threads_and_handoffs() {
    let brain = ScriptedBrain::new()
        .with_turn(
            "lead",
            vec![
                BotAction::CreateChannel {
                    name: "design".to_owned(),
                    purpose: "UX".to_owned(),
                    members: vec!["ada".to_owned()],
                },
                BotAction::Post {
                    text: "kickoff".to_owned(),
                    channel: Some("design".to_owned()),
                    thread: None,
                },
                BotAction::Handoff {
                    to: "ada".to_owned(),
                    summary: "own the flags".to_owned(),
                },
                BotAction::Post {
                    text: "nope".to_owned(),
                    channel: Some("missing".to_owned()),
                    thread: None,
                },
            ],
        )
        .with_turn(
            "ada",
            vec![BotAction::Post {
                text: "on it".to_owned(),
                channel: Some("design".to_owned()),
                thread: Some(3),
            }],
        );
    let team = squad()
        .with_policy(FloorPolicyKind::MentionDriven)
        .with_budget(Budget {
            max_rounds: 2,
            ..Budget::default()
        });
    let (_, events) = run(team, scripted(brain)).await;
    let state = replay(&events);
    let design = state.channel("design").expect("channel");
    assert!(design.includes("lead") && design.includes("ada") && !design.includes("rev"));
    let kickoff = state
        .messages
        .iter()
        .find(|m| m.text == "kickoff")
        .expect("kickoff");
    let reply = state
        .messages
        .iter()
        .find(|m| m.text == "on it")
        .expect("reply");
    assert_eq!(reply.thread, Some(kickoff.id));
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.kind == MessageKind::Handoff && m.mentions == vec!["ada".to_owned()])
    );
    assert_eq!(state.stats["lead"].denied, 1);
}

struct SlowBrain;

#[async_trait]
impl BotBrain for SlowBrain {
    async fn take_turn(
        &self,
        _context: TurnContext,
        _cancellation: CancellationToken,
    ) -> Result<BotTurn, TeamError> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(BotTurn::pass())
    }
}

#[tokio::test]
async fn slow_turns_time_out_and_are_reported() {
    let mut team = TeamSpec::new("t", vec![bot("lead", "lead")]);
    team.budget = Budget {
        max_rounds: 1,
        turn_timeout_secs: Some(0),
        ..Budget::default()
    };
    let (report, events) = run(team, BrainRouter::new(Arc::new(SlowBrain))).await;
    assert_eq!(report.members["lead"].failures, 1);
    assert!(events.iter().any(
        |event| matches!(event, TeamEvent::TurnFailed { error, .. } if error.contains("timed out"))
    ));
}

#[tokio::test]
async fn brain_router_overrides_individual_bots() {
    let router = BrainRouter::new(Arc::new(ScriptedBrain::new())).with_bot(
        "lead",
        Arc::new(ScriptedBrain::new().with_turn("lead", vec![BotAction::post("custom brain")])),
    );
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let (_, events) = run(team, router).await;
    assert!(
        replay(&events)
            .messages
            .iter()
            .any(|m| m.text == "custom brain")
    );
}

fn panel() -> TeamSpec {
    TeamSpec::new(
        "panel",
        vec![
            bot("lead", "lead"),
            bot("r1", "reviewer"),
            bot("r2", "reviewer"),
            bot("r3", "reviewer"),
        ],
    )
    .with_approvers(["r1", "r2", "r3"])
}

#[tokio::test]
async fn majority_rule_settles_before_every_vote_is_cast() {
    let brain = ScriptedBrain::new()
        .with_turn(
            "lead",
            vec![BotAction::ProposeCompletion {
                summary: "done".to_owned(),
            }],
        )
        .with_turn(
            "r1",
            vec![BotAction::Vote {
                approve: true,
                reason: "ok".to_owned(),
            }],
        )
        .with_turn(
            "r2",
            vec![BotAction::Vote {
                approve: true,
                reason: "ok".to_owned(),
            }],
        )
        .with_turn(
            "r3",
            vec![BotAction::Vote {
                approve: false,
                reason: "never asked".to_owned(),
            }],
        );
    let team = panel().with_approval(nexus_teams::ApprovalRule::Majority);
    let (report, events) = run(team, scripted(brain)).await;
    assert_eq!(report.status, MissionStatus::Completed, "{report:?}");
    assert_eq!(
        report.members["r3"].turns, 0,
        "the outcome was decided before r3 spoke"
    );
    assert!(report.reason.contains("@r1") && report.reason.contains("@r2"));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TeamEvent::ActionDenied { .. }))
    );
}

#[tokio::test]
async fn any_rule_accepts_after_a_rejection() {
    let brain = ScriptedBrain::new()
        .with_turn(
            "lead",
            vec![BotAction::ProposeCompletion {
                summary: "done".to_owned(),
            }],
        )
        .with_turn(
            "r1",
            vec![BotAction::Vote {
                approve: false,
                reason: "nit".to_owned(),
            }],
        )
        .with_turn(
            "r2",
            vec![BotAction::Vote {
                approve: true,
                reason: "good".to_owned(),
            }],
        );
    let team = panel().with_approval(nexus_teams::ApprovalRule::Any);
    let (report, _) = run(team, scripted(brain)).await;
    assert_eq!(report.status, MissionStatus::Completed, "{report:?}");
    assert_eq!(report.members["r3"].turns, 0);
}

#[tokio::test]
async fn all_rule_rejects_on_first_objection_with_reasons() {
    let brain = ScriptedBrain::new()
        .with_turn(
            "lead",
            vec![BotAction::ProposeCompletion {
                summary: "done".to_owned(),
            }],
        )
        .with_turn(
            "r1",
            vec![BotAction::Vote {
                approve: false,
                reason: "missing docs".to_owned(),
            }],
        );
    let team = panel().with_budget(Budget {
        max_rounds: 2,
        stall_rounds: 0,
        ..Budget::default()
    });
    let (_, events) = run(team, scripted(brain)).await;
    assert!(events.iter().any(|event| matches!(
        event,
        TeamEvent::StatusChanged { status: MissionStatus::Active, reason }
            if reason.contains("`all` rule") && reason.contains("missing docs")
    )));
    let state = replay(&events);
    assert_eq!(
        state.stats["r2"].turns, 0,
        "remaining voters are skipped once rejected"
    );
}

#[tokio::test]
async fn human_direct_messages_open_private_channels() {
    let brain = ScriptedBrain::new().with_turn("ada", vec![BotAction::post("on it, privately")]);
    let team = squad()
        .with_policy(FloorPolicyKind::MentionDriven)
        .with_budget(Budget {
            max_rounds: 1,
            ..Budget::default()
        });
    let sink = Arc::new(MemoryEventSink::default());
    let engine = MissionEngine::new(team, Goal::new("g"), scripted(brain))
        .expect("engine")
        .with_sink(sink.clone());
    engine
        .control()
        .direct("@ada", "can you look at the flaky test?")
        .expect("queued");
    engine.run().await.expect("report");
    let state = replay(&sink.events());
    let dm = state.channel("dm:ada+human").expect("dm channel");
    assert!(dm.direct && dm.includes("human") && dm.includes("ada") && !dm.includes("lead"));
    let reply = state
        .messages
        .iter()
        .find(|m| m.text == "on it, privately")
        .expect("reply");
    assert_eq!(reply.channel, "dm:ada+human");
    assert_eq!(reply.kind, MessageKind::Direct);
    assert_eq!(
        state
            .visible_to("lead")
            .filter(|m| m.channel == dm.name)
            .count(),
        0
    );
}

#[tokio::test]
async fn unknown_direct_message_targets_are_reported() {
    let team = squad().with_budget(Budget {
        max_rounds: 1,
        ..Budget::default()
    });
    let sink = Arc::new(MemoryEventSink::default());
    let engine = MissionEngine::new(team, Goal::new("g"), scripted(ScriptedBrain::new()))
        .expect("engine")
        .with_sink(sink.clone());
    engine.control().direct("ghost", "hello?").expect("queued");
    engine.run().await.expect("report");
    let state = replay(&sink.events());
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.author == "system" && m.text.contains("@ghost"))
    );
}

#[tokio::test]
async fn completed_missions_reopen_only_with_follow_up_feedback() {
    let sink = Arc::new(MemoryEventSink::default());
    let first = MissionEngine::new(squad(), Goal::new("Ship it"), simulated())
        .expect("engine")
        .with_sink(sink.clone())
        .run()
        .await
        .expect("first run");
    assert_eq!(first.status, MissionStatus::Completed);

    let completed = replay(&sink.events());
    assert!(matches!(
        MissionEngine::resume(
            completed.clone(),
            simulated(),
            nexus_teams::ResumeOptions::default()
        ),
        Err(nexus_teams::TeamError::AlreadyCompleted)
    ));

    let reopened = MissionEngine::resume(
        completed,
        simulated(),
        nexus_teams::ResumeOptions {
            rounds: Some(8),
            note: Some("Also add a review checklist for the release".to_owned()),
            ..nexus_teams::ResumeOptions::default()
        },
    )
    .expect("reopen with feedback")
    .with_sink(sink.clone())
    .run()
    .await
    .expect("second run");
    assert_eq!(reopened.status, MissionStatus::Completed, "{reopened:?}");
    assert_eq!(reopened.tasks_total, first.tasks_total + 1);
    assert_eq!(reopened.tasks_done, reopened.tasks_total);

    let state = replay(&sink.events());
    let follow_up = state
        .board
        .tasks()
        .find(|task| task.title.starts_with("Follow-up:"))
        .expect("follow-up task");
    assert_eq!(follow_up.created_by, "lead");
    assert_eq!(
        follow_up.assignee.as_deref(),
        Some("rev"),
        "routed by expertise (review)"
    );
    assert_eq!(follow_up.status, nexus_teams::TaskStatus::Done);
}

#[tokio::test]
async fn missions_resume_with_fresh_budget_and_replay_end_to_end() {
    let tight = squad().with_budget(Budget {
        max_rounds: 2,
        stall_rounds: 0,
        ..Budget::default()
    });
    let sink = Arc::new(MemoryEventSink::default());
    let first = MissionEngine::new(tight, Goal::new("Ship it"), simulated())
        .expect("engine")
        .with_sink(sink.clone())
        .run()
        .await
        .expect("first run");
    assert_eq!(first.status, MissionStatus::OutOfBudget);

    let state = replay(&sink.events());
    let resumed = MissionEngine::resume(
        state,
        simulated(),
        nexus_teams::ResumeOptions {
            rounds: Some(10),
            note: Some("@lead please wrap this up".to_owned()),
            ..nexus_teams::ResumeOptions::default()
        },
    )
    .expect("resume")
    .with_sink(sink.clone())
    .run()
    .await
    .expect("second run");
    assert_eq!(resumed.status, MissionStatus::Completed, "{resumed:?}");
    assert_eq!(resumed.mission, first.mission, "same mission id");
    assert!(resumed.rounds > first.rounds);

    let events = sink.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, TeamEvent::MissionStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, TeamEvent::MissionResumed { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, TeamEvent::MissionFinished { .. }))
            .count(),
        2
    );
    let state = replay(&events);
    assert_eq!(state.status, MissionStatus::Completed);
    assert_eq!(state.team.budget.max_rounds, first.rounds + 10);
    assert_eq!(state.report.as_ref(), Some(&resumed));
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.author == "human" && m.text.contains("wrap this up"))
    );
}

#[tokio::test]
async fn built_in_templates_complete_with_simulated_brains() {
    let library =
        nexus_teams::TeamLibrary::new(std::env::temp_dir().join("nexus-engine-templates"));
    for template in nexus_teams::TeamTemplate::all() {
        let team = library
            .resolve_team(template.team_file(template.key))
            .expect("template team");
        if team.requires_human_approval() {
            let result = MissionEngine::new(team, Goal::new("g"), simulated())
                .expect("engine")
                .run()
                .await;
            assert!(
                matches!(result, Err(TeamError::InvalidTeam(_))),
                "{}",
                template.key
            );
            continue;
        }
        let (report, _) = run(team, simulated()).await;
        assert_eq!(
            report.status,
            MissionStatus::Completed,
            "{}: {report:?}",
            template.key
        );
    }
}
