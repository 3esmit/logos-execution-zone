//! Lenient, deserialize-only model of a Grafana panel, for the panel→Rust
//! transpiler (`codegen`).
//!
//! A real Grafana export is far wider than what we emit: `unit` may be absent,
//! `options.tooltip.sort` may be `"none"`, overrides carry property types we
//! don't model, and there are dozens of fields we ignore. So this model is
//! deliberately separate from the strict `schema` (sized for *output*): it
//! captures only what codegen reads, makes every field optional, and lets serde
//! drop everything else — including the whole `options` block, which codegen
//! reconstructs from the panel type rather than reading.

use serde::Deserialize;
use serde_json::Value;

use crate::schema::{
    AxisPlacement, Color, GradientMode, LineInterpolation, PanelType, ShowPoints, StackingMode,
    Thresholds,
};

#[derive(Deserialize)]
pub struct PanelInput {
    #[serde(rename = "type")]
    pub panel_type: PanelType,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "gridPos", default)]
    pub grid_pos: GridPos,
    #[serde(rename = "fieldConfig", default)]
    pub field_config: FieldConfig,
    #[serde(default)]
    pub targets: Vec<Target>,
}

#[derive(Deserialize, Default)]
pub struct GridPos {
    #[serde(default)]
    pub w: u32,
}

#[derive(Deserialize, Default)]
pub struct FieldConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub overrides: Vec<Override>,
}

#[derive(Deserialize, Default)]
pub struct Defaults {
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub decimals: Option<u32>,
    #[serde(default)]
    pub color: Option<Color>,
    #[serde(default)]
    pub custom: Option<Custom>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub thresholds: Option<Thresholds>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Custom {
    #[serde(default)]
    pub span_nulls: Option<bool>,
    #[serde(default)]
    pub line_interpolation: Option<LineInterpolation>,
    #[serde(default)]
    pub show_points: Option<ShowPoints>,
    #[serde(default)]
    pub gradient_mode: Option<GradientMode>,
    #[serde(default)]
    pub stacking: Option<Stacking>,
    #[serde(default)]
    pub axis_placement: Option<AxisPlacement>,
    #[serde(default)]
    pub axis_label: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Stacking {
    #[serde(default)]
    pub mode: Option<StackingMode>,
}

#[derive(Deserialize)]
pub struct Override {
    #[serde(default)]
    pub matcher: Matcher,
    #[serde(default)]
    pub properties: Vec<Property>,
}

#[derive(Deserialize, Default)]
pub struct Matcher {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub options: Value,
}

impl Matcher {
    /// The series name for a `byName` matcher; `None` for matcher kinds the
    /// builder can't express (which the caller skips).
    pub fn by_name(&self) -> Option<&str> {
        if self.id == "byName" {
            self.options.as_str()
        } else {
            None
        }
    }
}

/// One override property. `id`/`value` are kept raw so unknown kinds are simply
/// ignored by codegen rather than failing the whole parse.
#[derive(Deserialize)]
pub struct Property {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Deserialize, Default)]
pub struct Target {
    #[serde(default)]
    pub expr: String,
    #[serde(rename = "legendFormat", default)]
    pub legend: String,
}
