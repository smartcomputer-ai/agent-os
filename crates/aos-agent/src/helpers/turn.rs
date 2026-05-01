use crate::contracts::{
    ActiveWindowItem, ContextOperationPhase, ContextOperationState, HostSessionStatus, RunCause,
    RunConfig, RunId, SessionId, SessionTurnState, ToolExecutor, ToolMapper, ToolRuntimeContext,
    ToolSpec, TurnBudget, TurnInput, TurnInputKind, TurnInputLane, TurnPlan, TurnPrerequisite,
    TurnPrerequisiteKind, TurnPriority, TurnReport, TurnStateUpdate, TurnTokenEstimate,
    TurnToolChoice, TurnToolInput,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct TurnRequest<'a> {
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub run_cause: Option<&'a RunCause>,
    pub run_config: &'a RunConfig,
    pub budget: TurnBudget,
    pub state: &'a SessionTurnState,
    pub inputs: &'a [TurnInput],
    pub tools: &'a [TurnToolInput],
    pub registry: &'a BTreeMap<String, ToolSpec>,
    pub profiles: &'a BTreeMap<String, Vec<String>>,
    pub runtime: &'a ToolRuntimeContext,
    pub pending_context_operation: Option<&'a ContextOperationState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPlanError {
    EmptySelection,
    UnknownTool,
}

pub trait TurnPlanner {
    fn build_turn(&self, request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultTurnPlanner;

impl TurnPlanner for DefaultTurnPlanner {
    fn build_turn(&self, request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError> {
        build_default_turn_plan(request)
    }
}

pub fn build_default_turn_plan(request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError> {
    let mut all_inputs = Vec::new();
    all_inputs.extend(request.state.pinned_inputs.iter().cloned());
    all_inputs.extend(request.state.durable_inputs.iter().cloned());
    all_inputs.extend(request.inputs.iter().cloned());
    all_inputs.sort_by_key(|input| {
        (
            input_kind_rank(&input.kind),
            lane_rank(&input.lane),
            priority_rank(&input.priority),
        )
    });

    let mut active_window_items = Vec::new();
    let mut response_format_ref = None;
    let mut provider_options_ref = None;
    let mut seen_refs = BTreeSet::new();
    let mut selected_message_count = 0_u64;
    let mut dropped_message_count = 0_u64;
    let mut message_tokens = 0_u64;
    let mut unknown_message_count = 0_u64;
    let mut decision_codes = Vec::new();
    let mut unresolved = Vec::new();

    for input in all_inputs {
        match input.kind {
            TurnInputKind::MessageRef => {
                if !seen_refs.insert(input.content_ref.clone()) {
                    dropped_message_count = dropped_message_count.saturating_add(1);
                    decision_codes.push(format!("drop_message_duplicate:{}", input.input_id));
                    continue;
                }
                let required = matches!(input.priority, TurnPriority::Required);
                if over_message_ref_budget(
                    active_window_items.len(),
                    request.budget.max_message_refs,
                ) && !required
                {
                    dropped_message_count = dropped_message_count.saturating_add(1);
                    decision_codes.push(format!("drop_message_ref_budget:{}", input.input_id));
                    continue;
                }
                if over_token_budget(
                    message_tokens,
                    input.estimated_tokens,
                    request.budget.max_input_tokens,
                ) && !required
                {
                    dropped_message_count = dropped_message_count.saturating_add(1);
                    decision_codes.push(format!("drop_message_token_budget:{}", input.input_id));
                    continue;
                }
                if let Some(tokens) = input.estimated_tokens {
                    message_tokens = message_tokens.saturating_add(tokens);
                } else {
                    unknown_message_count = unknown_message_count.saturating_add(1);
                }
                selected_message_count = selected_message_count.saturating_add(1);
                active_window_items.push(ActiveWindowItem::message_ref(
                    input.input_id,
                    input.content_ref,
                    Some(input.lane),
                    input.estimated_tokens,
                    None,
                ));
            }
            TurnInputKind::ResponseFormatRef => {
                if response_format_ref.is_none() {
                    response_format_ref = Some(input.content_ref);
                    decision_codes.push("select_response_format".into());
                }
            }
            TurnInputKind::ProviderOptionsRef => {
                if provider_options_ref.is_none() {
                    provider_options_ref = Some(input.content_ref);
                    decision_codes.push("select_provider_options".into());
                }
            }
            TurnInputKind::ArtifactRef | TurnInputKind::Custom { .. } => {
                decision_codes.push(format!("drop_non_message_input:{}", input.input_id));
            }
        }
    }

    if active_window_items.is_empty() {
        return Err(TurnPlanError::EmptySelection);
    }

    let (selected_tool_ids, dropped_tool_count, tool_tokens, unknown_tool_count, mut prerequisites) =
        select_tools(
            &request,
            message_tokens,
            &mut decision_codes,
            &mut unresolved,
        )?;
    if let Some(prerequisite) = context_operation_prerequisite(request.pending_context_operation) {
        unresolved.push("context_operation_pending".into());
        prerequisites.push(prerequisite);
    }
    let selected_tool_count = selected_tool_ids.len() as u64;

    if !prerequisites.is_empty() {
        unresolved.push("prerequisites_pending".into());
    }

    let token_estimate = TurnTokenEstimate {
        message_tokens,
        tool_tokens,
        total_input_tokens: message_tokens.saturating_add(tool_tokens),
        unknown_message_count,
        unknown_tool_count,
    };
    decision_codes.push(format!(
        "selected_turn:session={}:run={}",
        request.session_id.0, request.run_id.run_seq
    ));

    Ok(TurnPlan {
        active_window_items,
        tool_choice: if selected_tool_ids.is_empty() {
            None
        } else {
            Some(TurnToolChoice::Auto)
        },
        selected_tool_ids,
        response_format_ref,
        provider_options_ref,
        prerequisites,
        state_updates: Vec::new(),
        report: TurnReport {
            planner: "aos.agent/default-turn".into(),
            selected_message_count,
            dropped_message_count,
            selected_tool_count,
            dropped_tool_count,
            token_estimate,
            budget: request.budget,
            decision_codes,
            unresolved,
        },
    })
}

fn context_operation_prerequisite(
    operation: Option<&ContextOperationState>,
) -> Option<TurnPrerequisite> {
    let operation = operation?;
    if !operation.blocks_generation() {
        return None;
    }
    let kind = if matches!(operation.phase, ContextOperationPhase::CountingTokens) {
        TurnPrerequisiteKind::CountTokens
    } else {
        TurnPrerequisiteKind::CompactContext
    };
    Some(TurnPrerequisite {
        prerequisite_id: format!("context_operation:{}", operation.operation_id),
        kind,
        reason: format!(
            "context operation '{}' is {}",
            operation.operation_id,
            operation.phase.as_str()
        ),
        input_ids: Vec::new(),
        tool_ids: Vec::new(),
    })
}

fn select_tools(
    request: &TurnRequest<'_>,
    message_tokens: u64,
    decision_codes: &mut Vec<String>,
    unresolved: &mut Vec<String>,
) -> Result<(Vec<String>, u64, u64, u64, Vec<TurnPrerequisite>), TurnPlanError> {
    let mut selected = Vec::new();
    let mut dropped = 0_u64;
    let mut tool_tokens = 0_u64;
    let mut unknown_tools = 0_u64;
    let mut seen = BTreeSet::new();
    let mut host_blocked = Vec::new();
    let host_ready = request.runtime.host_session_status == Some(HostSessionStatus::Ready);

    for tool in request.tools {
        if !seen.insert(tool.tool_id.clone()) {
            dropped = dropped.saturating_add(1);
            decision_codes.push(format!("drop_tool_duplicate:{}", tool.tool_id));
            continue;
        }
        let Some(spec) = request.registry.get(&tool.tool_id) else {
            unresolved.push(format!("unknown_tool:{}", tool.tool_id));
            return Err(TurnPlanError::UnknownTool);
        };
        let required = matches!(tool.priority, TurnPriority::Required);
        if tool_requires_host_session(spec) && !host_ready {
            dropped = dropped.saturating_add(1);
            host_blocked.push(tool.tool_id.clone());
            decision_codes.push(format!("drop_tool_host_not_ready:{}", tool.tool_id));
            continue;
        }
        if over_tool_ref_budget(selected.len(), request.budget.max_tool_refs) && !required {
            dropped = dropped.saturating_add(1);
            decision_codes.push(format!("drop_tool_ref_budget:{}", tool.tool_id));
            continue;
        }
        if over_token_budget(
            message_tokens.saturating_add(tool_tokens),
            tool.estimated_tokens,
            request.budget.max_input_tokens,
        ) && !required
        {
            dropped = dropped.saturating_add(1);
            decision_codes.push(format!("drop_tool_token_budget:{}", tool.tool_id));
            continue;
        }
        if let Some(tokens) = tool.estimated_tokens {
            tool_tokens = tool_tokens.saturating_add(tokens);
        } else {
            unknown_tools = unknown_tools.saturating_add(1);
        }
        selected.push(tool.tool_id.clone());
    }

    let prerequisites =
        if !host_blocked.is_empty() && request.run_config.host_session_open.is_some() {
            vec![TurnPrerequisite {
                prerequisite_id: "host_session:open".into(),
                kind: TurnPrerequisiteKind::OpenHostSession,
                reason: "selected tool candidates require a ready host session".into(),
                input_ids: Vec::new(),
                tool_ids: host_blocked,
            }]
        } else {
            if !host_blocked.is_empty() {
                unresolved.push("host_session_not_ready".into());
            }
            Vec::new()
        };

    Ok((selected, dropped, tool_tokens, unknown_tools, prerequisites))
}

fn input_kind_rank(kind: &TurnInputKind) -> u8 {
    match kind {
        TurnInputKind::ProviderOptionsRef => 0,
        TurnInputKind::ResponseFormatRef => 1,
        TurnInputKind::MessageRef => 2,
        TurnInputKind::ArtifactRef => 3,
        TurnInputKind::Custom { .. } => 4,
    }
}

fn lane_rank(lane: &TurnInputLane) -> u8 {
    match lane {
        TurnInputLane::System => 0,
        TurnInputLane::Developer => 1,
        TurnInputLane::Summary => 2,
        TurnInputLane::Skill => 3,
        TurnInputLane::Memory => 4,
        TurnInputLane::Domain => 5,
        TurnInputLane::RuntimeHint => 6,
        TurnInputLane::Conversation => 7,
        TurnInputLane::ToolResult => 8,
        TurnInputLane::Steer => 9,
        TurnInputLane::Custom { .. } => 10,
    }
}

fn priority_rank(priority: &TurnPriority) -> u8 {
    match priority {
        TurnPriority::Required => 0,
        TurnPriority::High => 1,
        TurnPriority::Normal => 2,
        TurnPriority::Low => 3,
    }
}

fn over_message_ref_budget(current_len: usize, max: Option<u64>) -> bool {
    max.is_some_and(|max| current_len >= max as usize)
}

fn over_tool_ref_budget(current_len: usize, max: Option<u64>) -> bool {
    max.is_some_and(|max| current_len >= max as usize)
}

fn over_token_budget(current_tokens: u64, candidate: Option<u64>, max: Option<u64>) -> bool {
    match (candidate, max) {
        (Some(candidate), Some(max)) => current_tokens.saturating_add(candidate) > max,
        _ => false,
    }
}

pub fn tool_requires_host_session(spec: &ToolSpec) -> bool {
    if matches!(spec.executor, ToolExecutor::HostLoop { .. }) {
        return false;
    }
    matches!(
        spec.mapper,
        ToolMapper::HostExec
            | ToolMapper::HostSessionSignal
            | ToolMapper::HostFsReadFile
            | ToolMapper::HostFsWriteFile
            | ToolMapper::HostFsEditFile
            | ToolMapper::HostFsApplyPatch
            | ToolMapper::HostFsGrep
            | ToolMapper::HostFsGlob
            | ToolMapper::HostFsStat
            | ToolMapper::HostFsExists
            | ToolMapper::HostFsListDir
    )
}

pub fn apply_turn_state_updates(state: &mut SessionTurnState, updates: &[TurnStateUpdate]) {
    for update in updates {
        match update {
            TurnStateUpdate::UpsertPinnedInput(input) => {
                state
                    .pinned_inputs
                    .retain(|existing| existing.input_id != input.input_id);
                state.pinned_inputs.push(input.clone());
            }
            TurnStateUpdate::RemovePinnedInput { input_id } => {
                state
                    .pinned_inputs
                    .retain(|existing| existing.input_id != *input_id);
            }
            TurnStateUpdate::UpsertDurableInput(input) => {
                state
                    .durable_inputs
                    .retain(|existing| existing.input_id != input.input_id);
                state.durable_inputs.push(input.clone());
            }
            TurnStateUpdate::RemoveDurableInput { input_id } => {
                state
                    .durable_inputs
                    .retain(|existing| existing.input_id != *input_id);
            }
            TurnStateUpdate::UpsertCustomStateRef(value) => {
                state.custom_state_refs.retain(|existing| {
                    existing.planner_id != value.planner_id || existing.key != value.key
                });
                state.custom_state_refs.push(value.clone());
            }
            TurnStateUpdate::RemoveCustomStateRef { planner_id, key } => {
                state
                    .custom_state_refs
                    .retain(|existing| existing.planner_id != *planner_id || existing.key != *key);
            }
            TurnStateUpdate::Noop => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        CompactionStrategy, ContextPressureReason, HostSessionOpenConfig, HostTargetConfig,
        ToolExecutor,
    };
    use alloc::boxed::Box;
    use alloc::vec;

    fn hash(seed: char) -> String {
        let mut value = String::from("sha256:");
        let nibble = b"0123456789abcdef"[seed as usize % 16] as char;
        for _ in 0..64 {
            value.push(nibble);
        }
        value
    }

    fn message(id: &str, lane: TurnInputLane, priority: TurnPriority, seed: char) -> TurnInput {
        TurnInput {
            input_id: id.into(),
            lane,
            kind: TurnInputKind::MessageRef,
            priority,
            content_ref: hash(seed),
            estimated_tokens: Some(10),
            source_kind: None,
            source_id: None,
            correlation_id: None,
            tags: Vec::new(),
        }
    }

    fn request<'a>(
        inputs: &'a [TurnInput],
        tools: &'a [TurnToolInput],
        registry: &'a BTreeMap<String, ToolSpec>,
        run_config: &'a RunConfig,
        runtime: &'a ToolRuntimeContext,
        budget: TurnBudget,
    ) -> TurnRequest<'a> {
        static SESSION: SessionId = SessionId(String::new());
        static RUN: RunId = RunId {
            session_id: SessionId(String::new()),
            run_seq: 1,
        };
        TurnRequest {
            session_id: &SESSION,
            run_id: &RUN,
            run_cause: None,
            run_config,
            budget,
            state: Box::leak(Box::new(SessionTurnState {
                pinned_inputs: Vec::new(),
                durable_inputs: Vec::new(),
                last_report: None,
                custom_state_refs: Vec::new(),
            })),
            inputs,
            tools,
            registry,
            profiles: Box::leak(Box::new(BTreeMap::new())),
            runtime,
            pending_context_operation: None,
        }
    }

    #[test]
    fn default_planner_orders_stable_lanes_before_conversation() {
        let inputs = vec![
            message(
                "turn",
                TurnInputLane::Conversation,
                TurnPriority::Required,
                'c',
            ),
            message("system", TurnInputLane::System, TurnPriority::Required, 'a'),
            message("summary", TurnInputLane::Summary, TurnPriority::High, 'b'),
        ];

        let plan = build_default_turn_plan(request(
            &inputs,
            &[],
            &BTreeMap::new(),
            &RunConfig::default(),
            &ToolRuntimeContext::default(),
            TurnBudget::default(),
        ))
        .expect("plan");

        assert_eq!(
            plan.active_window_items
                .iter()
                .map(|item| item.ref_.clone())
                .collect::<Vec<_>>(),
            vec![hash('a'), hash('b'), hash('c')]
        );
        assert_eq!(plan.report.selected_message_count, 3);
    }

    #[test]
    fn default_planner_reports_token_budget_drops_and_unknowns() {
        let mut unknown = message(
            "unknown",
            TurnInputLane::Conversation,
            TurnPriority::Normal,
            'b',
        );
        unknown.estimated_tokens = None;
        let inputs = vec![
            message(
                "required",
                TurnInputLane::System,
                TurnPriority::Required,
                'a',
            ),
            message(
                "drop",
                TurnInputLane::Conversation,
                TurnPriority::Normal,
                'c',
            ),
            unknown,
        ];

        let plan = build_default_turn_plan(request(
            &inputs,
            &[],
            &BTreeMap::new(),
            &RunConfig::default(),
            &ToolRuntimeContext::default(),
            TurnBudget {
                max_input_tokens: Some(15),
                ..TurnBudget::default()
            },
        ))
        .expect("plan");

        assert_eq!(
            plan.active_window_items
                .iter()
                .map(|item| item.ref_.clone())
                .collect::<Vec<_>>(),
            vec![hash('a'), hash('b')]
        );
        assert_eq!(plan.report.dropped_message_count, 1);
        assert_eq!(plan.report.token_estimate.unknown_message_count, 1);
    }

    #[test]
    fn default_planner_blocks_host_tools_until_session_ready() {
        let registry = BTreeMap::from([(
            "host.exec".into(),
            ToolSpec {
                tool_id: "host.exec".into(),
                tool_name: "shell".into(),
                tool_ref: hash('f'),
                description: String::new(),
                args_schema_json: "{}".into(),
                mapper: ToolMapper::HostExec,
                executor: ToolExecutor::Effect {
                    effect: "sys/host.exec@1".into(),
                },
                parallelism_hint: Default::default(),
            },
        )]);
        let tools = vec![TurnToolInput {
            tool_id: "host.exec".into(),
            priority: TurnPriority::Normal,
            estimated_tokens: Some(8),
            source_kind: None,
            source_id: None,
            tags: Vec::new(),
        }];
        let run_config = RunConfig {
            host_session_open: Some(HostSessionOpenConfig {
                target: HostTargetConfig::default(),
                session_ttl_ns: None,
                labels: None,
            }),
            ..RunConfig::default()
        };

        let plan = build_default_turn_plan(request(
            &[message(
                "turn",
                TurnInputLane::Conversation,
                TurnPriority::Required,
                'a',
            )],
            &tools,
            &registry,
            &run_config,
            &ToolRuntimeContext::default(),
            TurnBudget::default(),
        ))
        .expect("plan");

        assert!(plan.selected_tool_ids.is_empty());
        assert!(matches!(
            plan.prerequisites.first().map(|value| &value.kind),
            Some(TurnPrerequisiteKind::OpenHostSession)
        ));
    }

    #[test]
    fn default_planner_maps_context_operation_to_prerequisite() {
        let mut operation = ContextOperationState::needs_compaction(
            "ctx-op-1",
            ContextPressureReason::UsageHighWater,
            CompactionStrategy::AosSummary,
            None,
            0,
        );
        let inputs = vec![message(
            "turn",
            TurnInputLane::Conversation,
            TurnPriority::Required,
            'a',
        )];
        let registry = BTreeMap::new();
        let run_config = RunConfig::default();
        let runtime = ToolRuntimeContext::default();
        let mut compact_request = request(
            &inputs,
            &[],
            &registry,
            &run_config,
            &runtime,
            TurnBudget::default(),
        );
        compact_request.pending_context_operation = Some(&operation);
        let plan = build_default_turn_plan(compact_request).expect("plan");
        assert!(matches!(
            plan.prerequisites.first().map(|value| &value.kind),
            Some(TurnPrerequisiteKind::CompactContext)
        ));

        operation.phase = ContextOperationPhase::CountingTokens;
        let mut count_request = request(
            &inputs,
            &[],
            &registry,
            &run_config,
            &runtime,
            TurnBudget::default(),
        );
        count_request.pending_context_operation = Some(&operation);
        let plan = build_default_turn_plan(count_request).expect("plan");
        assert!(matches!(
            plan.prerequisites.first().map(|value| &value.kind),
            Some(TurnPrerequisiteKind::CountTokens)
        ));
    }
}
