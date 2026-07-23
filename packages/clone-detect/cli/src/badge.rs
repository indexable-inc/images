use snafu::ResultExt as _;

use crate::{BadgeWriteSnafu, RunError};

const GREEN_THRESHOLD: f64 = 5.0;
const YELLOW_THRESHOLD: f64 = 15.0;
const ORANGE_THRESHOLD: f64 = 30.0;

const COLOR_GREEN: &str = "#4c1";
const COLOR_YELLOW: &str = "#dfb317";
const COLOR_ORANGE: &str = "#fe7d37";
const COLOR_RED: &str = "#e05d44";

const CHAR_WIDTH: usize = 7;
const PADDING: usize = 10;
const SVG_SCALE: f64 = 10.0;
const HALF: f64 = 2.0;

#[expect(
    clippy::cast_precision_loss,
    reason = "badge widths are small positive integers, exact in f64"
)]
pub fn write(path: &std::path::Path, pct: f64) -> Result<(), RunError> {
    let label = "duplication";
    let value = format!("{pct:.1}%");

    let color = if pct < GREEN_THRESHOLD {
        COLOR_GREEN
    } else if pct < YELLOW_THRESHOLD {
        COLOR_YELLOW
    } else if pct < ORANGE_THRESHOLD {
        COLOR_ORANGE
    } else {
        COLOR_RED
    };

    let label_width = CHAR_WIDTH * label.len() + PADDING;
    let value_width = CHAR_WIDTH * value.len() + PADDING;
    let total_width = label_width + value_width;
    let label_x = label_width as f64 / HALF;
    let value_x = label_width as f64 + value_width as f64 / HALF;

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "SVG coordinates are always small positive values"
    )]
    let label_x_10 = (label_x * SVG_SCALE) as u32;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "SVG coordinates are always small positive values"
    )]
    let value_x_10 = (value_x * SVG_SCALE) as u32;

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total_width}" height="20" role="img" aria-label="{label}: {value}">
  <title>{label}: {value}</title>
  <linearGradient id="s" x2="0" y2="100%">
    <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
    <stop offset="1" stop-opacity=".1"/>
  </linearGradient>
  <clipPath id="r"><rect width="{total_width}" height="20" rx="3" fill="#fff"/></clipPath>
  <g clip-path="url(#r)">
    <rect width="{label_width}" height="20" fill="#555"/>
    <rect x="{label_width}" width="{value_width}" height="20" fill="{color}"/>
    <rect width="{total_width}" height="20" fill="url(#s)"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" text-rendering="geometricPrecision" font-size="110">
    <text aria-hidden="true" x="{label_x_10}" y="150" fill="#010101" fill-opacity=".3" transform="scale(.1)">{label}</text>
    <text x="{label_x_10}" y="140" transform="scale(.1)">{label}</text>
    <text aria-hidden="true" x="{value_x_10}" y="150" fill="#010101" fill-opacity=".3" transform="scale(.1)">{value}</text>
    <text x="{value_x_10}" y="140" transform="scale(.1)">{value}</text>
  </g>
</svg>"##,
    );

    std::fs::write(path, svg).context(BadgeWriteSnafu { path })?;
    Ok(())
}
