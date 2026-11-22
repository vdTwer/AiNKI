//! The stats charts, drawn on the server as SVG. Colors come from
//! style.css through classes, so dark mode and e-ink follow. Every mark has
//! a `<title>`, so hovering (or a screen reader) gives its numbers without
//! JavaScript.

use maud::{Markup, html};

use crate::num;

/// One chart's box in SVG units; it scales to the column.
const W: f64 = 640.0;
const H: f64 = 200.0;
/// Room for the axis labels left and below.
const LEFT: f64 = 40.0;
const BOTTOM: f64 = 24.0;
const TOP: f64 = 18.0;

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn svg(label: &str, body: &Markup) -> Markup {
    html! {
        svg.chart viewBox=(format!("0 0 {W} {H}")) role="img" aria-label=(label) { (body) }
    }
}

/// Recall week by week: (answers, remembered) per week, oldest first, with
/// the aim as a dashed line. Weeks without answers leave a gap.
pub fn retention(weeks: &[(i64, i64)], aim: Option<f64>) -> Markup {
    let vals: Vec<Option<f64>> =
        weeks.iter().map(|&(n, p)| (n > 0).then(|| num::i64_f64(p) / num::i64_f64(n))).collect();
    let seen: Vec<f64> = vals.iter().flatten().copied().chain(aim).collect();
    if vals.iter().all(Option::is_none) {
        return html! { p.muted.small { "No reviews of learned cards in the last twelve weeks." } };
    }
    // The floor sits just under the lowest value, on a 5% step.
    let low = seen.iter().copied().fold(1.0, f64::min);
    let lo = (((low - 0.03) * 20.0).floor() / 20.0).clamp(0.0, 0.8);
    let last = vals.len().saturating_sub(1).max(1);
    let x = |i: usize| LEFT + num::usize_f64(i) * (W - LEFT - 8.0) / num::usize_f64(last);
    let y = |v: f64| ((1.0 - v) / (1.0 - lo)).mul_add(H - TOP - BOTTOM, TOP);
    let mut path = String::new();
    let mut pen = false;
    for (i, v) in vals.iter().enumerate() {
        match v {
            Some(v) => {
                path.push_str(&format!("{}{:.1} {:.1}", if pen { 'L' } else { 'M' }, x(i), y(*v)));
                pen = true;
            }
            None => pen = false,
        }
    }
    let grid: Vec<f64> = (0..=20).map(|k| f64::from(k) * 0.05).filter(|g| *g >= lo - 1e-9).collect();
    svg(
        "Recall by week, last twelve weeks",
        &html! {
            @for g in &grid {
                line.grid x1=(LEFT) x2=(W - 8.0) y1=(y(*g)) y2=(y(*g)) {}
                @if (g * 10.0).fract().abs() < 1e-6 || (g * 10.0).fract() > 1.0 - 1e-6 {
                    text.axis x="0" y=(y(*g) + 4.0) { (pct(*g)) }
                }
            }
            @if let Some(a) = aim {
                line.aim x1=(LEFT) x2=(W - 8.0) y1=(y(a)) y2=(y(a)) {}
                text.axis.strong x=(LEFT + 56.0) y=(y(a) - 6.0) { "aim " (pct(a)) }
            }
            path.line d=(path) {}
            @for (i, (v, &(n, p))) in vals.iter().zip(weeks).enumerate() {
                @if let Some(v) = v {
                    @let ago = last - i;
                    circle.dot cx=(x(i)) cy=(y(*v)) r="3.5" {
                        title { (week_name(ago)) ": " (pct(*v)) ", " (p) " of " (n) " remembered" }
                    }
                }
            }
            text.axis x=(LEFT) y=(H - 4.0) { "12 weeks ago" }
            text.axis x=(W - 8.0) y=(H - 4.0) text-anchor="end" { "this week" }
        },
    )
}

fn week_name(ago: usize) -> String {
    match ago {
        0 => "This week".into(),
        1 => "Last week".into(),
        n => format!("{n} weeks ago"),
    }
}

/// Expected against remembered: one dot per group of reviews, on a square
/// where the diagonal means FSRS predicted exactly.
pub fn calibration(bins: &[(f64, f64, i64)]) -> Markup {
    const S: f64 = 200.0;
    const PAD: f64 = 28.0;
    let p = |v: f64| v.mul_add(S - PAD - 6.0, PAD);
    let q = |v: f64| v.mul_add(-(S - PAD - 6.0), S - PAD);
    html! {
        svg.chart.calibration viewBox=(format!("0 0 {S} {S}")) role="img" aria-label="Expected against remembered, last year" {
            line.grid x1=(p(0.0)) y1=(q(0.0)) x2=(p(1.0)) y2=(q(0.0)) {}
            line.grid x1=(p(0.0)) y1=(q(0.0)) x2=(p(0.0)) y2=(q(1.0)) {}
            line.aim x1=(p(0.0)) y1=(q(0.0)) x2=(p(1.0)) y2=(q(1.0)) {}
            text.axis x=(p(0.5)) y=(S - 6.0) text-anchor="middle" { "expected" }
            text.axis x="10" y=(q(0.5)) text-anchor="middle" transform=(format!("rotate(-90 10 {})", q(0.5))) { "remembered" }
            @for &(predicted, actual, reviews) in bins {
                circle.dot cx=(p(predicted)) cy=(q(actual)) r="4" {
                    title { "Expected " (pct(predicted)) ", remembered " (pct(actual)) " (" (reviews) " reviews)" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_leaves_gaps_and_titles_each_week() {
        let svg = retention(&[(10, 9), (0, 0), (20, 18)], Some(0.9)).into_string();
        assert_eq!(svg.matches('M').count(), 2, "a week without answers breaks the line: {svg}");
        assert!(svg.contains("<title>This week: 90%, 18 of 20 remembered</title>"));
        assert!(svg.contains("aim 90%"));
    }

    #[test]
    fn retention_without_answers_says_so() {
        assert!(retention(&[(0, 0); 12], None).into_string().contains("No reviews"));
    }

}
