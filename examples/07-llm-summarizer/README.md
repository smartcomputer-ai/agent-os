# Example 07 — LLM Summarizer

This demo wires together an HTTP fetch with a mocked LLM call to produce a
summary. The plan fetches a document, hands it to a deterministic LLM harness,
then reports the summary and token usage back to the reducer.

* Reducer: `demo/LlmSummarizer@1` (tracks requests and summaries)
* Plan: `demo/summarize_plan@1` (HTTP → LLM → event)
* Capabilities: `demo/http_fetch_cap@1`, `demo/llm_summarize_cap@1`
* Policy: `demo/llm-policy@1` allowing `llm.generate` only from the plan

Run it with:

```
cargo run -p aos-examples -- llm-summarizer
```
