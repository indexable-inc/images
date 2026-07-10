//! Self-contained HTML report of the journal's run history: one DAG per run,
//! nodes colored by what happened (with the state always written out in
//! text), plus per-run tiles and invalidation reasons. No external assets.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use efx_engine::journal::{Action, JournalState, RunRecord};

const NODE_W: i64 = 200;
const NODE_H: i64 = 58;
const GAP_X: i64 = 72;
const GAP_Y: i64 = 26;
const PAD: i64 = 24;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const fn action_parts(action: Action) -> ActionStyle {
    match action {
        Action::Executed => ActionStyle {
            class: "executed",
            label: "executed",
        },
        Action::Cached => ActionStyle {
            class: "cached",
            label: "cache hit",
        },
        Action::Failed => ActionStyle {
            class: "failed",
            label: "failed",
        },
        Action::Skipped => ActionStyle {
            class: "skipped",
            label: "skipped",
        },
    }
}

struct ActionStyle {
    class: &'static str,
    label: &'static str,
}

/// Unix seconds to `YYYY-MM-DD HH:MM:SS UTC` (Howard Hinnant's civil-from-days).
fn format_utc(secs: u64) -> String {
    // Never fails: u64::MAX / 86_400 (~2.1e14) is far below i64::MAX, and a
    // silent `unwrap_or(0)` would trip clippy::fallible_int_fallback anyway.
    let days = i64::try_from(secs / 86_400).expect("u64 day count fits in i64");
    let rem = secs % 86_400;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

struct Position {
    column: i64,
    row: i64,
}

/// Longest-path column per node, rows stacked per column in record order.
fn layout(run: &RunRecord) -> BTreeMap<&str, Position> {
    let mut depth: BTreeMap<&str, i64> = BTreeMap::new();
    // Effects are recorded in topological order, so one forward pass settles
    // longest-path depths.
    for effect in &run.effects {
        let mine = run
            .edges
            .iter()
            .filter(|edge| edge.to == effect.name)
            .filter_map(|edge| depth.get(edge.from.as_str()).map(|d| d + 1))
            .max()
            .unwrap_or(0);
        depth.insert(&effect.name, mine);
    }
    let mut rows: BTreeMap<i64, i64> = BTreeMap::new();
    let mut positions = BTreeMap::new();
    for effect in &run.effects {
        let column = depth[effect.name.as_str()];
        let row = rows.entry(column).or_insert(0);
        positions.insert(effect.name.as_str(), Position { column, row: *row });
        *row += 1;
    }
    positions
}

fn render_svg(run: &RunRecord) -> String {
    let positions = layout(run);
    let columns = positions.values().map(|p| p.column).max().unwrap_or(0) + 1;
    let tallest = positions
        .values()
        .fold(BTreeMap::<i64, i64>::new(), |mut acc, p| {
            let count = acc.entry(p.column).or_insert(0);
            *count = (*count).max(p.row + 1);
            acc
        })
        .into_values()
        .max()
        .unwrap_or(1);
    let width = columns * (NODE_W + GAP_X) - GAP_X + 2 * PAD;
    let height = tallest * (NODE_H + GAP_Y) - GAP_Y + 2 * PAD;
    let x_of = |p: &Position| PAD + p.column * (NODE_W + GAP_X);
    let y_of = |p: &Position| PAD + p.row * (NODE_H + GAP_Y);

    let mut svg = String::new();
    let _ = write!(
        svg,
        r#"<svg viewBox="0 0 {width} {height}" width="{width}" height="{height}" role="img" aria-label="effect DAG">"#
    );
    for edge in &run.edges {
        let (Some(from), Some(to)) = (
            positions.get(edge.from.as_str()),
            positions.get(edge.to.as_str()),
        ) else {
            continue;
        };
        let x1 = x_of(from) + NODE_W;
        let y1 = y_of(from) + NODE_H / 2;
        let x2 = x_of(to);
        let y2 = y_of(to) + NODE_H / 2;
        let bend = GAP_X / 2;
        let _ = write!(
            svg,
            r#"<path class="edge" d="M {x1} {y1} C {} {y1}, {} {y2}, {} {y2} L {x2} {y2}" />"#,
            x1 + bend,
            x2 - bend,
            x2 - 6
        );
        let _ = write!(
            svg,
            r#"<path class="arrow" d="M {} {} L {x2} {y2} L {} {} Z" />"#,
            x2 - 7,
            y2 - 4,
            x2 - 7,
            y2 + 4
        );
    }
    for effect in &run.effects {
        let position = &positions[effect.name.as_str()];
        let x = x_of(position);
        let y = y_of(position);
        let style = action_parts(effect.action);
        let detail = match effect.action {
            Action::Executed => format!("{} · {}ms", style.label, effect.duration_ms),
            _ => style.label.to_owned(),
        };
        let tooltip = effect.reason.as_deref().unwrap_or("unchanged");
        let _ = write!(
            svg,
            concat!(
                r#"<g class="node {}"><title>{} — {}: {}</title>"#,
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" class="box" />"#,
                r#"<rect x="{x}" y="{y}" width="4" height="{h}" rx="2" class="accent" />"#,
                r#"<text x="{tx}" y="{ty1}" class="name">{}</text>"#,
                r#"<text x="{tx}" y="{ty2}" class="meta">{} · {}</text>"#,
                "</g>"
            ),
            style.class,
            escape(&effect.name),
            style.label,
            escape(tooltip),
            escape(&effect.name),
            escape(&effect.kind),
            escape(&detail),
            x = x,
            y = y,
            w = NODE_W,
            h = NODE_H,
            tx = x + 16,
            ty1 = y + 24,
            ty2 = y + 43,
        );
    }
    svg.push_str("</svg>");
    svg
}

fn render_run(index: usize, run: &RunRecord) -> String {
    let count = |action: Action| run.effects.iter().filter(|e| e.action == action).count();
    let mut html = String::new();
    let _ = write!(
        html,
        r#"<section class="run"><header><h2>run {}</h2><span class="when">{}</span></header>"#,
        index + 1,
        format_utc(run.recorded_at)
    );
    html.push_str(r#"<div class="tiles">"#);
    for (action, hint) in [
        (Action::Executed, "did real work"),
        (Action::Cached, "memoized, skipped"),
        (Action::Failed, "executor error"),
        (Action::Skipped, "blocked by a failure"),
    ] {
        let style = action_parts(action);
        let _ = write!(
            html,
            concat!(
                r#"<div class="tile {}" title="{}"><span class="dot"></span>"#,
                r#"<span class="value">{}</span><span class="label">{}</span></div>"#
            ),
            style.class,
            hint,
            count(action),
            style.label
        );
    }
    html.push_str("</div>");
    let _ = write!(html, r#"<div class="dag">{}</div>"#, render_svg(run));

    let reasons: Vec<&_> = run
        .effects
        .iter()
        .filter(|e| e.action != Action::Cached)
        .collect();
    if reasons.is_empty() {
        html.push_str(r#"<p class="allcached">every effect was a cache hit — nothing to do</p>"#);
    } else {
        html.push_str(r#"<table class="reasons"><thead><tr><th>effect</th><th>state</th><th>why</th></tr></thead><tbody>"#);
        for effect in reasons {
            let style = action_parts(effect.action);
            let _ = write!(
                html,
                r#"<tr><td class="ename">{}</td><td><span class="badge {}">{}</span></td><td>{}</td></tr>"#,
                escape(&effect.name),
                style.class,
                style.label,
                escape(effect.reason.as_deref().unwrap_or("—"))
            );
        }
        html.push_str("</tbody></table>");
    }
    html.push_str("</section>");
    html
}

/// Renders the whole journal as one self-contained HTML page.
#[must_use]
pub fn render(state: &JournalState) -> String {
    let mut body = String::new();
    if state.runs.is_empty() {
        body.push_str(
            "<p class=\"allcached\">no runs recorded yet — run <code>efx apply</code> first</p>",
        );
    }
    for (index, run) in state.runs.iter().enumerate() {
        body.push_str(&render_run(index, run));
    }
    let mut legend = String::new();
    for action in [
        Action::Executed,
        Action::Cached,
        Action::Failed,
        Action::Skipped,
    ] {
        let style = action_parts(action);
        let _ = write!(
            legend,
            r#"<span class="key {}"><span class="dot"></span>{}</span>"#,
            style.class, style.label
        );
    }
    include_str!("report_template.html")
        .replace("%%RUNS%%", &state.runs.len().to_string())
        .replace("%%ENTRIES%%", &state.entries.len().to_string())
        .replace("%%LEGEND%%", &legend)
        .replace("%%BODY%%", &body)
}
