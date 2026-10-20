//! Preview-only history page builders for control IPC.
use crate::{
    adapters::{ClipboardData, ClipboardEntry},
    control::schema::{
        HistoryEntryDetail, HistoryEntryView, HistoryPage, HISTORY_PAGE_MAX, HISTORY_PREVIEW_CHARS,
        HISTORY_QUERY_MAX_BYTES,
    },
};
use anyhow::{bail, Result};
use uuid::Uuid;

pub fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(1, HISTORY_PAGE_MAX)
}

pub fn validate_query(query: &str) -> Result<()> {
    if query.len() > HISTORY_QUERY_MAX_BYTES {
        bail!("History query too long");
    }
    Ok(())
}

/// Fold newlines and other control characters to spaces; truncate to preview budget.
pub fn preview_text(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let count = folded.chars().count();
    if count <= HISTORY_PREVIEW_CHARS {
        return folded;
    }
    folded.chars().take(HISTORY_PREVIEW_CHARS).collect()
}

fn entry_text(entry: &ClipboardEntry) -> Option<&str> {
    match &entry.content {
        ClipboardData::Text(text) => Some(text.as_str()),
    }
}

fn content_type(_entry: &ClipboardEntry) -> &'static str {
    "text/plain"
}

pub fn to_view(entry: &ClipboardEntry) -> Option<HistoryEntryView> {
    let text = entry_text(entry)?;
    Some(HistoryEntryView {
        id: entry.id,
        timestamp: entry.timestamp.to_rfc3339(),
        origin_node: entry.source,
        content_type: content_type(entry).into(),
        content_chars: text.chars().count() as u32,
        preview: preview_text(text),
    })
}

pub fn to_detail(entry: &ClipboardEntry, max_bytes: usize) -> Result<HistoryEntryDetail> {
    let text = entry_text(entry)
        .ok_or_else(|| anyhow::anyhow!("History entry not found"))?
        .to_string();
    if text.len() > max_bytes {
        bail!("Clipboard exceeds configured size limit");
    }
    let view = to_view(entry).ok_or_else(|| anyhow::anyhow!("History entry not found"))?;
    Ok(HistoryEntryDetail {
        id: view.id,
        timestamp: view.timestamp,
        origin_node: view.origin_node,
        content_type: view.content_type,
        content_chars: view.content_chars,
        preview: view.preview,
        text,
    })
}

pub fn page_from_entries(
    entries: &[ClipboardEntry],
    query: Option<String>,
    limit: u32,
) -> HistoryPage {
    let limit = clamp_limit(limit) as usize;
    let mut views = Vec::new();
    let mut truncated = false;
    for entry in entries.iter().take(limit) {
        if let Some(view) = to_view(entry) {
            if view.preview.chars().count()
                < entry_text(entry).map(|t| t.chars().count()).unwrap_or(0)
            {
                truncated = true;
            }
            views.push(view);
        }
    }
    HistoryPage {
        query,
        total_returned: views.len() as u32,
        truncated,
        entries: views,
    }
}

pub fn missing_id_message(_id: Uuid) -> &'static str {
    "History entry not found"
}
