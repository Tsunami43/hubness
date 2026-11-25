//! Markdown → styled lines (agent replies): headings, emphasis, inline code, lists, quotes,
//! links and fenced code blocks with syntax highlighting.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::Theme;
use super::code::highlight;

pub fn render(text: &str) -> Vec<Line<'static>> {
    let t = Theme::get();
    let mut out: Vec<Line<'static>> = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut styles: Vec<Style> = vec![Style::default()];
    let mut list: Vec<Option<u64>> = vec![];
    let mut quote = 0usize;
    let mut code: Option<(String, String)> = None;
    let mut link: Option<String> = None;

    let flush = |cur: &mut Vec<Span<'static>>, out: &mut Vec<Line<'static>>, quote: usize| {
        if !cur.is_empty() {
            let mut spans = vec![];
            if quote > 0 {
                spans.push(Span::styled("▎ ".repeat(quote), t.muted_style()));
            }
            spans.append(cur);
            out.push(Line::from(spans));
        }
    };
    let style = |styles: &Vec<Style>| *styles.last().unwrap_or(&Style::default());

    for ev in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS) {
        if let Some((lang, buf)) = &mut code {
            match ev {
                Event::Text(s) => buf.push_str(&s),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, buf) = (lang.clone(), buf.clone());
                    code = None;
                    for l in highlight(&buf, &lang) {
                        let mut spans = vec![Span::styled("  ", Style::default())];
                        spans.extend(l.spans);
                        out.push(Line::from(spans));
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut cur, &mut out, quote);
                    let s =
                        Style::default().add_modifier(Modifier::BOLD).fg(if level == HeadingLevel::H1 { t.accent } else { Color::Reset });
                    styles.push(s);
                }
                Tag::Emphasis => styles.push(style(&styles).add_modifier(Modifier::ITALIC)),
                Tag::Strong => styles.push(style(&styles).add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => styles.push(style(&styles).add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { dest_url, .. } => {
                    link = Some(dest_url.to_string());
                    styles.push(style(&styles).fg(t.info).add_modifier(Modifier::UNDERLINED));
                }
                Tag::BlockQuote(_) => {
                    flush(&mut cur, &mut out, quote);
                    quote += 1;
                }
                Tag::List(start) => {
                    flush(&mut cur, &mut out, quote);
                    list.push(start);
                }
                Tag::Item => {
                    flush(&mut cur, &mut out, quote);
                    let depth = list.len().saturating_sub(1);
                    let bullet = match list.last_mut() {
                        Some(Some(n)) => {
                            let b = format!("{n}. ");
                            *n += 1;
                            b
                        }
                        _ => "• ".into(),
                    };
                    cur.push(Span::styled(format!("{}{bullet}", "  ".repeat(depth)), t.accent_style()));
                }
                Tag::CodeBlock(kind) => {
                    flush(&mut cur, &mut out, quote);
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::Paragraph => {}
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Heading(_) | TagEnd::Paragraph => {
                    if matches!(end, TagEnd::Heading(_)) {
                        styles.pop();
                    }
                    flush(&mut cur, &mut out, quote);
                    if list.is_empty() {
                        out.push(Line::raw(""));
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    styles.pop();
                }
                TagEnd::Link => {
                    styles.pop();
                    if let Some(url) = link.take() {
                        cur.push(Span::styled(format!(" ({url})"), t.muted_style()));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut cur, &mut out, quote);
                    quote = quote.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    flush(&mut cur, &mut out, quote);
                    list.pop();
                    if list.is_empty() {
                        out.push(Line::raw(""));
                    }
                }
                TagEnd::Item => flush(&mut cur, &mut out, quote),
                _ => {}
            },
            Event::Text(s) => {
                let st = style(&styles);
                let mut first = true;
                for part in s.split('\n') {
                    if !first {
                        flush(&mut cur, &mut out, quote);
                    }
                    first = false;
                    if !part.is_empty() {
                        cur.push(Span::styled(part.to_string(), st));
                    }
                }
            }
            Event::Code(s) => cur.push(Span::styled(s.to_string(), Style::default().fg(t.warn))),
            Event::SoftBreak => cur.push(Span::raw(" ")),
            Event::HardBreak => flush(&mut cur, &mut out, quote),
            Event::Rule => {
                flush(&mut cur, &mut out, quote);
                out.push(Line::styled("─".repeat(40), t.muted_style()));
            }
            Event::TaskListMarker(done) => cur.push(Span::styled(if done { "☑ " } else { "☐ " }, t.accent_style())),
            _ => {}
        }
    }
    flush(&mut cur, &mut out, quote);
    while out.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn renders_common_markdown() {
        let lines = render("# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n\n```rust\nfn main() {}\n```");
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect()).collect();
        assert_eq!(text[0], "Title");
        assert!(text.iter().any(|l| l == "Some bold and code."));
        assert!(text.iter().any(|l| l == "• one"));
        assert!(text.iter().any(|l| l.contains("fn main()")));
    }
}
