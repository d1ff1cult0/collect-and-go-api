//! Server-side SVG-trapgrafiek van de prijsgeschiedenis.
//! Een prijs geldt van zijn eerste waarneming tot de volgende wijziging, dus een trapfunctie.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::pages::{fmt_date_short, fmt_euro};
use crate::db::PriceRow;

const W: f64 = 760.0;
const H: f64 = 280.0;
const PAD_L: f64 = 72.0;
const PAD_R: f64 = 20.0;
const PAD_T: f64 = 16.0;
const PAD_B: f64 = 36.0;

#[derive(Serialize)]
pub struct ChartPoint {
    /// x-positie in viewBox-eenheden
    x: f64,
    y: f64,
    /// einde van dit segment (x)
    x2: f64,
    price: String,
    from: String,
    until: String,
}

pub struct Chart {
    pub svg: String,
    pub points_json: String,
}

fn nice_step(range: f64, target_ticks: f64) -> f64 {
    let raw = (range / target_ticks).max(0.01);
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render(history: &[PriceRow], now: DateTime<Utc>) -> Option<Chart> {
    // `now` dient enkel als terugvalwaarde voor ongeldige tijdstempels.
    let first = history.first()?;
    let last = history.last()?;
    let t0 = first.first_seen_at.timestamp() as f64;
    // Laat de lijn doorlopen tot de laatste waarneming (of minstens een dag na de start).
    let mut t1 = last.last_seen_at.timestamp() as f64;
    if t1 - t0 < 86_400.0 {
        t1 = t0 + 86_400.0;
    }

    let prices: Vec<f64> = history
        .iter()
        .map(|r| r.base_price as f64 / 100.0)
        .collect();
    let (mut lo, mut hi) = prices
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &p| (a.min(p), b.max(p)));
    if (hi - lo).abs() < 0.005 {
        lo = (lo * 0.8).max(0.0);
        hi *= 1.15;
    }
    let step = nice_step(hi - lo, 4.0);
    let y_min = ((lo - step * 0.5) / step).floor().max(0.0) * step;
    let y_max = ((hi + step * 0.3) / step).ceil() * step;

    let x = |t: f64| PAD_L + (t - t0) / (t1 - t0) * (W - PAD_L - PAD_R);
    let y = |p: f64| PAD_T + (1.0 - (p - y_min) / (y_max - y_min)) * (H - PAD_T - PAD_B);

    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg class="chart-svg" viewBox="0 0 {W} {H}" role="img" aria-labelledby="chart-title chart-desc" tabindex="0">"#
    ));

    // Rasterlijnen + y-labels
    let mut v = y_min;
    while v <= y_max + 1e-9 {
        let yy = y(v);
        svg.push_str(&format!(
            r#"<line class="grid" x1="{PAD_L}" x2="{}" y1="{yy:.1}" y2="{yy:.1}"/><text class="tick" x="{}" y="{:.1}" text-anchor="end">€ {}</text>"#,
            W - PAD_R,
            PAD_L - 8.0,
            yy + 4.0,
            fmt_euro((v * 100.0).round() as i64)
        ));
        v += step;
    }

    // x-labels: ±5 datums gelijk verdeeld
    let n_ticks = 5;
    for i in 0..n_ticks {
        let t = t0 + (t1 - t0) * i as f64 / (n_ticks - 1) as f64;
        let anchor = match i {
            0 => "start",
            i if i == n_ticks - 1 => "end",
            _ => "middle",
        };
        let dt = DateTime::<Utc>::from_timestamp(t as i64, 0).unwrap_or(now);
        svg.push_str(&format!(
            r#"<text class="tick" x="{:.1}" y="{}" text-anchor="{anchor}">{}</text>"#,
            x(t),
            H - 10.0,
            esc(&fmt_date_short(dt))
        ));
    }
    svg.push_str(&format!(
        r#"<line class="axis" x1="{PAD_L}" x2="{}" y1="{}" y2="{}"/>"#,
        W - PAD_R,
        H - PAD_B,
        H - PAD_B
    ));

    // Trapfunctie
    let mut points = Vec::new();
    let mut d = String::new();
    for (i, r) in history.iter().enumerate() {
        let start = r.first_seen_at.timestamp() as f64;
        let end = history
            .get(i + 1)
            .map(|n| n.first_seen_at.timestamp() as f64)
            .unwrap_or(t1);
        let p = prices[i];
        let (xs, xe, yy) = (x(start), x(end), y(p));
        if i == 0 {
            d.push_str(&format!("M{xs:.1},{yy:.1}"));
        } else {
            d.push_str(&format!(" V{yy:.1}"));
        }
        d.push_str(&format!(" H{xe:.1}"));
        points.push(ChartPoint {
            x: (xs * 10.0).round() / 10.0,
            y: (yy * 10.0).round() / 10.0,
            x2: (xe * 10.0).round() / 10.0,
            price: format!("€ {}", fmt_euro(r.base_price)),
            from: super::pages::fmt_datetime(r.first_seen_at),
            until: super::pages::fmt_datetime(if i + 1 < history.len() {
                history[i + 1].first_seen_at
            } else {
                r.last_seen_at
            }),
        });
    }
    let base_y = H - PAD_B;
    let area = format!("{d} V{base_y:.1} H{:.1} Z", x(t0));
    svg.push_str(&format!(r#"<path class="area" d="{area}"/>"#));
    svg.push_str(&format!(r#"<path class="line" d="{d}"/>"#));
    // Eindpunt
    let lp = points.last().unwrap();
    svg.push_str(&format!(
        r#"<circle class="end-dot" cx="{:.1}" cy="{:.1}" r="4.5"/>"#,
        lp.x2, lp.y
    ));
    // Interactielaag (crosshair + punt), gestuurd door chart.js
    svg.push_str(&format!(
        r#"<g class="hover" visibility="hidden"><line class="crosshair" y1="{PAD_T}" y2="{base_y}"/><circle class="hover-dot" r="4.5"/></g><rect class="hit" x="{PAD_L}" y="0" width="{}" height="{H}"/>"#,
        W - PAD_L - PAD_R
    ));
    svg.push_str("</svg>");

    Some(Chart {
        svg,
        points_json: serde_json::to_string(&points).unwrap_or_else(|_| "[]".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(nice_step(1.0, 4.0), 0.25);
        assert_eq!(nice_step(10.0, 4.0), 2.5);
        assert_eq!(nice_step(0.3, 4.0), 0.1);
    }

    #[test]
    fn renders_history() {
        let t = Utc::now();
        let row = |id, price, from: DateTime<Utc>, until: DateTime<Utc>| PriceRow {
            id,
            product_id: "1".into(),
            base_price: price,
            unit_price: None,
            offer_price: None,
            offer_quantity: None,
            offer_unit_price: None,
            reference_price: None,
            is_red_price: false,
            first_seen_at: from,
            last_seen_at: until,
        };
        let h = vec![
            row(
                1,
                319,
                t - chrono::Duration::days(20),
                t - chrono::Duration::days(10),
            ),
            row(2, 299, t - chrono::Duration::days(9), t),
        ];
        let c = render(&h, t).unwrap();
        assert!(c.svg.contains("<path class=\"line\""));
        assert!(c.points_json.contains("€ 2,99"));
        assert!(render(&[], t).is_none());
    }
}
