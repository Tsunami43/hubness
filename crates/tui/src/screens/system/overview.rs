//! System → Overview: host, CPU, memory, GPUs, sensors, load, uptime and disks, refreshed
//! every few seconds while visible, with CPU / memory history sparklines.

use std::collections::VecDeque;
use std::time::Duration;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline, Wrap};
use serde_json::Value;

use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::kit::{Ticker, bar, fmt_uptime, gb, num, pct_color, s};
use crate::ui::{Theme, block};

const EVERY: Duration = Duration::from_secs(5);
const HISTORY: usize = 120;

pub struct Overview {
    data: Value,
    error: Option<String>,
    ticker: Ticker,
    cpu: VecDeque<u64>,
    mem: VecDeque<u64>,
}

impl Default for Overview {
    fn default() -> Self {
        Self { data: Value::Null, error: None, ticker: Ticker::new(EVERY), cpu: VecDeque::new(), mem: VecDeque::new() }
    }
}

fn push(q: &mut VecDeque<u64>, v: Option<f64>) {
    if let Some(v) = v {
        q.push_back(v.clamp(0.0, 100.0).round() as u64);
        while q.len() > HISTORY {
            q.pop_front();
        }
    }
}

fn temp_style(c: f64) -> Style {
    let t = Theme::get();
    Style::default().fg(if c >= 85.0 {
        t.danger
    } else if c >= 70.0 {
        t.warn
    } else {
        ratatui::style::Color::Reset
    })
}

impl Overview {
    fn load(&self, ctx: &Ctx) {
        ctx.get("overview", "/system/overview");
    }

    fn tile(f: &mut Frame, area: Rect, title: &str, value: Line<'static>, pct: Option<f64>, sub: Vec<Line<'static>>) {
        let b = block(title, false);
        let inner = b.inner(area);
        f.render_widget(b, area);
        let mut lines = vec![value.style(Theme::get().bold())];
        if let Some(p) = pct {
            lines.push(Line::from(bar(p, inner.width as usize)));
        }
        lines.extend(sub);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }
}

impl Screen for Overview {
    fn title(&self) -> String {
        "Overview".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.load(ctx);
        self.ticker.start(ctx, "tick");
    }

    fn actions(&self) -> Vec<Action> {
        vec![Action::new("r", "Refresh now")]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if key == "r" {
            self.load(ctx);
        }
    }

    fn on_key(&mut self, _k: KeyEvent, _ctx: &mut Ctx) -> Handled {
        Handled::No
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "tick" => {
                if self.ticker.tick(ctx, "tick") {
                    self.load(ctx);
                }
            }
            "overview" => match result {
                Ok(v) => {
                    push(&mut self.cpu, v["cpuPercent"].as_f64());
                    push(&mut self.mem, v["memory"]["usedPercent"].as_f64());
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        self.ticker.drawn();
        let t = Theme::get();
        let o = &self.data;
        if o.is_null() {
            let text = self.error.clone().unwrap_or_else(|| "Loading…".into());
            f.render_widget(Paragraph::new(text).block(block("Overview", false)), area);
            return;
        }
        let i = &o["info"];
        let unsupported = i["platform"] == "other";
        let [head, tiles, hist, disks] = Layout::vertical([
            Constraint::Length(if unsupported { 5 } else { 4 }),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Min(3),
        ])
        .areas(area);

        // host header
        let load = o["load"].as_array().map(|l| l.iter().map(num).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let machine = [s(&o["hostname"], ""), s(&i["host"], "")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("{} ", s(&i["osName"], "?")), t.bold()),
                Span::styled(s(&i["arch"], "").to_string(), t.muted_style()),
                Span::styled(format!("   {machine}"), t.muted_style()),
            ]),
            Line::from(vec![
                Span::raw(format!("{} · up {}", s(&i["kernel"], ""), fmt_uptime(o["uptimeSec"].as_u64().unwrap_or(0)))),
                Span::styled(format!("   load {load}"), t.muted_style()),
                Span::styled(format!("   refreshed every {}s", EVERY.as_secs()), t.muted_style()),
            ]),
        ];
        if unsupported {
            lines.push(Line::styled(
                "This OS is not fully supported: System pages target Linux distributions and macOS. Only basic metrics are shown.",
                Style::default().fg(t.warn),
            ));
        }
        if let Some(e) = &self.error {
            lines.push(Line::styled(e.clone(), Style::default().fg(t.danger)));
        }
        f.render_widget(Paragraph::new(lines).block(block("Host", false)), head);

        // tiles: CPU, RAM, GPUs, temperature
        let gpus = o["gpus"].as_array().cloned().unwrap_or_default();
        let temps = o["temps"].as_array().cloned().unwrap_or_default();
        let n = 2 + gpus.len() + usize::from(!temps.is_empty());
        let cells = Layout::horizontal(vec![Constraint::Ratio(1, n as u32); n]).split(tiles);
        let cpu = o["cpuPercent"].as_f64();
        let mut sub = vec![];
        if let Some(c) = i["cpu"].as_str() {
            sub.push(Line::styled(c.to_string(), t.muted_style()));
        }
        sub.push(Line::styled(format!("{} threads", num(&o["cpus"])), t.muted_style()));
        Self::tile(
            f,
            cells[0],
            "CPU",
            Line::raw(format!("{}%", cpu.map(|c| num(&c.into())).unwrap_or("—".into()))),
            Some(cpu.unwrap_or(0.0)),
            sub,
        );

        let m = &o["memory"];
        let (total, avail) = (m["totalMb"].as_f64().unwrap_or(0.0), m["availableMb"].as_f64().unwrap_or(0.0));
        let mut sub = vec![Line::styled(format!("{} / {} GB", gb(total - avail), gb(total)), t.muted_style())];
        let sw = &o["swap"];
        if sw["totalMb"].as_f64().unwrap_or(0.0) > 0.0 {
            sub.push(Line::styled(
                format!("swap {} / {} GB", gb(sw["usedMb"].as_f64().unwrap_or(0.0)), gb(sw["totalMb"].as_f64().unwrap_or(0.0))),
                t.muted_style(),
            ));
        }
        Self::tile(f, cells[1], "RAM", Line::raw(format!("{}%", num(&m["usedPercent"]))), m["usedPercent"].as_f64(), sub);

        for (k, g) in gpus.iter().enumerate() {
            let title = if gpus.len() > 1 { format!("GPU {}", k + 1) } else { "GPU".into() };
            let usage = g["usagePercent"].as_f64();
            let mut sub = vec![Line::styled(s(&g["name"], "").to_string(), t.muted_style())];
            if let Some(c) = g["tempC"].as_f64() {
                sub.push(Line::styled(format!("{}°C", c.round()), temp_style(c)));
            }
            let value = usage.map(|u| format!("{}%", num(&u.into()))).unwrap_or("—".into());
            Self::tile(f, cells[2 + k], &title, Line::raw(value), usage, sub);
        }
        if !temps.is_empty() {
            let main = temps.iter().find(|x| x["kind"] == "cpu").unwrap_or(&temps[0]);
            let c = main["celsius"].as_f64().unwrap_or(0.0);
            let sub = temps
                .iter()
                .map(|x| {
                    let c = x["celsius"].as_f64().unwrap_or(0.0);
                    Line::from(vec![
                        Span::styled(format!("{:<10}", s(&x["label"], "")), t.muted_style()),
                        Span::styled(format!("{}°C", c.round()), temp_style(c)),
                    ])
                })
                .collect();
            Self::tile(f, cells[n - 1], "Temperature", Line::styled(format!("{}°C", c.round()), temp_style(c)), None, sub);
        }

        // history
        let [hc, hm] = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(hist);
        for (area, title, data) in [(hc, "CPU history", &self.cpu), (hm, "Memory history", &self.mem)] {
            let b = block(format!("{title} (last {} min)", (HISTORY as u64 * EVERY.as_secs()) / 60), false);
            let width = b.inner(area).width as usize;
            let tail: Vec<u64> = data.iter().skip(data.len().saturating_sub(width)).copied().collect();
            let last = tail.last().copied().unwrap_or(0) as f64;
            f.render_widget(Sparkline::default().block(b).data(&tail).max(100).style(Style::default().fg(pct_color(last))), area);
        }

        // disks
        let b = block("Disks", false);
        let inner = b.inner(disks);
        f.render_widget(b, disks);
        let w = inner.width as usize;
        let mut lines = vec![];
        let list: Vec<&Value> = o["disks"].as_array().into_iter().flatten().collect();
        let width_of = |k: &str| list.iter().map(|d| s(&d[k], "").chars().count()).max().unwrap_or(0);
        let (mw, dw) = (width_of("mount"), width_of("device"));
        // the device column only when there is room for a useful bar too
        let device = w >= mw + dw + 50;
        let size_of = |d: &Value| format!("{} / {} GB", num(&d["usedGb"]), num(&d["sizeGb"]));
        let sw = list.iter().map(|d| size_of(d).chars().count()).max().unwrap_or(0);
        for d in list {
            let label = if device {
                format!(" {:<mw$}  {:<dw$} ", s(&d["mount"], ""), s(&d["device"], ""))
            } else {
                format!(" {:<mw$} ", s(&d["mount"], ""))
            };
            let size = format!(" {:>sw$} {:>4}% ", size_of(d), num(&d["usedPercent"]));
            let bar_w = w.saturating_sub(label.chars().count() + size.chars().count()).max(4);
            let mut spans = vec![Span::raw(label)];
            spans.extend(bar(d["usedPercent"].as_f64().unwrap_or(0.0), bar_w));
            spans.push(Span::raw(size));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
