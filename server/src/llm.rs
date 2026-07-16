//! Minimal wrapper around a single non-streaming Anthropic Messages API call
//! (P13), used to turn a `pdf::compare::CompareReport` into a short
//! natural-language summary of what changed between two documents.
//!
//! Degrades gracefully: returns `None` (not an error) whenever
//! `ANTHROPIC_API_KEY` is unset or the HTTP call fails, so `/compare` never
//! hard-fails just because no key is configured — the rest of the report
//! (page-by-page diff, stats) is still useful without it.
//!
//! The model is a deployment choice, not something to hardcode: read from
//! `ANTHROPIC_MODEL`, defaulting to Haiku 4.5 (cheap/fast, sufficient for
//! summarizing a bounded diff into a few sentences of prose). Point it at a
//! higher-tier model via the env var if summary quality on large/complex
//! diffs needs it.

use crate::pdf::compare::CompareReport;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-haiku-4-5";
/// Cap on how many per-page text-change excerpts go into the prompt, so a
/// huge diff doesn't blow up the token count (or the summary's own budget).
const MAX_EXCERPTS: usize = 50;
/// Bound a hung Anthropic call so `/compare` still returns the rest of the report.
const LLM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub async fn summarize_diff(report: &CompareReport) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let prompt = build_prompt(report);

    let client = match reqwest::Client::builder().timeout(LLM_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("LLM client build failed: {e}");
            return None;
        }
    };
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 700,
        "messages": [{ "role": "user", "content": prompt }],
    });

    let resp = match client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("LLM summary request failed: {e}");
            return None;
        }
    };

    if !resp.status().is_success() {
        tracing::warn!("LLM summary call returned {}", resp.status());
        return None;
    }

    let value: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("LLM summary response was not valid JSON: {e}");
            return None;
        }
    };
    value
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

fn build_prompt(report: &CompareReport) -> String {
    let mut excerpts = String::new();
    let mut count = 0;
    'pages: for page in &report.pages {
        for change in &page.text_changes {
            if count >= MAX_EXCERPTS {
                break 'pages;
            }
            let label = match change.kind {
                crate::pdf::compare::ChangeKind::Added => "新增",
                crate::pdf::compare::ChangeKind::Deleted => "刪除",
            };
            excerpts.push_str(&format!("- [{label}] {}\n", change.excerpt));
            count += 1;
        }
    }
    let truncated = if count >= MAX_EXCERPTS {
        "\n（差異片段過多，以上僅列出部分。）"
    } else {
        ""
    };

    format!(
        "以下是兩份 PDF 文件比對後的結構化差異資料：\n\n\
         原文件頁數：{}\n新文件頁數：{}\n\
         新增頁面：{} 頁，刪除頁面：{} 頁，內容變動頁面：{} 頁\n\
         文字差異片段：{}\n\n\
         差異片段列表：\n{}{}\n\n\
         請用 2 到 4 句話，以與上述差異片段相同的語言，簡潔說明這兩份文件之間的主要差異，\
         聚焦於實質內容變化（例如新增章節、刪除段落、數字或條款更動），不要條列逐項重述。",
        report.old_page_count,
        report.new_page_count,
        report.stats.pages_added,
        report.stats.pages_deleted,
        report.stats.pages_modified,
        report.stats.text_changes_total,
        excerpts,
        truncated,
    )
}
