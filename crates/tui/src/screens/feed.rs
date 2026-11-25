//! A session's event feed → styled lines (the web app's EventItem).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::ui::{Theme, code, markdown};

/// A short summary of a tool's input: the command, path, pattern or URL.
pub fn tool_summary(input: &Value) -> String {
    for k in ["command", "file_path", "pattern", "url", "path", "description", "query", "target"] {
        if let Some(s) = input[k].as_str() {
            return s.lines().next().unwrap_or_default().to_string();
        }
    }
    if let Some(todos) = input["todos"].as_array() {
        let done = todos.iter().filter(|t| t["status"] == "completed").count();
        return format!("{done}/{} done", todos.len());
    }
    String::new()
}

/// How much of a feed to show for each event.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Tool output collapsed, thinking shortened (the default).
    Compact,
    /// Everything: tool inputs and outputs, full thinking.
    Full,
}

pub fn event_lines(e: &Value, detail: Detail) -> Vec<Line<'static>> {
    let t = Theme::get();
    let text = |k: &str| e[k].as_str().unwrap_or_default().to_string();
    match e["kind"].as_str().unwrap_or_default() {
        "user" => {
            let mut out = vec![Line::raw("")];
            for (i, l) in text("text").lines().enumerate() {
                let prefix = if i == 0 { "› " } else { "  " };
                out.push(Line::styled(format!("{prefix}{l}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)));
            }
            if let Some(files) = e["attachments"].as_array() {
                for a in files {
                    out.push(Line::styled(
                        format!("  📎 {} ({})", a["name"].as_str().unwrap_or_default(), a["mime"].as_str().unwrap_or_default()),
                        t.accent_style(),
                    ));
                }
            }
            out
        }
        "text" => {
            let mut out = markdown::render(&text("text"));
            if e["streaming"] == true {
                out.push(Line::styled("▍", Style::default().fg(t.ok)));
            }
            out
        }
        "thinking" => {
            let take = if detail == Detail::Full { usize::MAX } else { 3 };
            let mut out: Vec<Line> = text("text")
                .lines()
                .take(take)
                .map(|l| Line::styled(format!("  ∴ {l}"), t.muted_style().add_modifier(Modifier::ITALIC)))
                .collect();
            if detail == Detail::Compact && text("text").lines().count() > 3 {
                out.push(Line::styled("    …", t.muted_style()));
            }
            out
        }
        "tool" => {
            let (mark, color) = match e["status"].as_str() {
                Some("ok") => ("✓", t.ok),
                Some("error") => ("✗", t.danger),
                Some("denied") => ("⊘", t.warn),
                _ => ("…", t.info),
            };
            let mut out = vec![Line::from(vec![
                Span::styled(format!("  {mark} "), Style::default().fg(color)),
                Span::styled(text("name"), t.bold()),
                Span::styled(format!(" {}", tool_summary(&e["input"])), t.muted_style()),
            ])];
            if detail == Detail::Full {
                let input = &e["input"];
                if e["name"] == "Edit" && input["old_string"].is_string() {
                    let diff: String = format!(
                        "{}\n{}",
                        input["old_string"].as_str().unwrap_or_default().lines().map(|l| format!("-{l}")).collect::<Vec<_>>().join("\n"),
                        input["new_string"].as_str().unwrap_or_default().lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n")
                    );
                    out.extend(code::diff(&diff).into_iter().map(indent));
                } else if !input.is_null() && input.as_object().is_some_and(|o| !o.is_empty()) {
                    out.extend(
                        serde_json::to_string_pretty(input)
                            .unwrap_or_default()
                            .lines()
                            .take(40)
                            .map(|l| Line::styled(format!("    {l}"), t.muted_style())),
                    );
                }
                if let Some(o) = e["output"].as_str().filter(|o| !o.is_empty()) {
                    out.extend(o.lines().take(60).map(|l| Line::styled(format!("    │ {l}"), t.muted_style())));
                }
            } else if e["status"] == "error"
                && let Some(o) = e["output"].as_str()
            {
                out.extend(o.lines().take(3).map(|l| Line::styled(format!("    {l}"), Style::default().fg(t.danger))));
            }
            out
        }
        "approval" => {
            let pending = e["status"] == "pending";
            let mut out = vec![Line::from(vec![
                Span::styled("  ? ", Style::default().fg(t.warn)),
                Span::styled(
                    format!("{} {}", text("toolName"), tool_summary(&e["input"])),
                    Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if pending { "  — Enter answer · y allow · n deny".to_string() } else { format!("  {}", text("status")) },
                    if pending { Style::default().fg(t.warn) } else { t.muted_style() },
                ),
            ])];
            if let Some(qs) = e["questions"].as_array() {
                for q in qs {
                    out.push(Line::styled(format!("    {}", q["question"].as_str().unwrap_or_default()), Style::default().fg(t.warn)));
                    for o in q["options"].as_array().into_iter().flatten() {
                        out.push(Line::styled(format!("      · {}", o["label"].as_str().unwrap_or_default()), t.muted_style()));
                    }
                }
            }
            if let Some(answers) = e["answers"].as_object() {
                for (q, a) in answers {
                    out.push(Line::styled(format!("    {q} → {}", a.as_str().unwrap_or_default()), t.muted_style()));
                }
            }
            if detail == Detail::Full && e["questions"].is_null() {
                out.extend(
                    serde_json::to_string_pretty(&e["input"])
                        .unwrap_or_default()
                        .lines()
                        .take(30)
                        .map(|l| Line::styled(format!("    {l}"), t.muted_style())),
                );
            }
            out
        }
        "notice" => {
            let style = if e["level"] == "error" { Style::default().fg(t.danger) } else { t.muted_style() };
            text("text").lines().map(|l| Line::styled(format!("  {l}"), style)).collect()
        }
        "result" => {
            let mut bits = vec![if e["ok"] == true {
                "done".to_string()
            } else if e["stopped"] == true {
                "stopped".into()
            } else {
                "failed".into()
            }];
            if let Some(ms) = e["durationMs"].as_f64() {
                bits.push(format!("{:.1}s", ms / 1000.0));
            }
            if e["billing"] == "api"
                && let Some(c) = e["costUsd"].as_f64()
            {
                bits.push(format!("${c:.2}"));
            }
            if let (Some(w), Some(u)) = (e["limit"]["window"].as_str(), e["limit"]["utilization"].as_f64()) {
                let pct = if u <= 1.0 { u * 100.0 } else { u };
                bits.push(format!("{w} {pct:.0}%"));
            }
            if let Some(m) = e["model"].as_str() {
                bits.push(m.to_string());
            }
            vec![Line::styled(format!("  — {}", bits.join(" · ")), t.muted_style())]
        }
        _ => vec![],
    }
}

fn indent(l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw("    ")];
    spans.extend(l.spans);
    Line::from(spans).style(l.style)
}
