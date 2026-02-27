mod llm_live_common;
mod llm_live_scenarios;
mod llm_live_tools;

use llm_live_common::require_live_matrix;
use llm_live_scenarios::{
    run_invalid_api_key, run_multi_turn_conversation, run_plain_completion, run_runtime_refs_smoke,
};
use llm_live_tools::{run_multi_tool_roundtrip, run_required_tool_call, run_tool_result_roundtrip};

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_plain_completion_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_plain_completion(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_required_tool_call_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_required_tool_call(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_tool_result_roundtrip_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_tool_result_roundtrip(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_multi_tool_roundtrip_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_multi_tool_roundtrip(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_multi_turn_conversation_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_multi_turn_conversation(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_runtime_refs_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_runtime_refs_smoke(case).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires RUN_LIVE_LLM_ADAPTER_TESTS=1 and provider keys (OPENAI_API_KEY and/or ANTHROPIC_API_KEY)"]
async fn llm_adapter_live_invalid_api_key_matrix() {
    let matrix = require_live_matrix();
    if matrix.is_empty() {
        return;
    }
    for case in &matrix {
        run_invalid_api_key(case).await;
    }
}
