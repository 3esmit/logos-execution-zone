//! The serializable Grafana dashboard schema — the internal data model the
//! public builders assemble into.
//!
//! Most types are `Serialize`-only, sized for what we *emit*. A handful of
//! vocabularies (`PanelType`, `Color`, and the styling enums) additionally
//! derive `Deserialize` because the lenient `input` model — which backs the
//! panel→Rust transpiler — reuses them.

use serde::{Deserialize, Serialize};

use crate::{DATASOURCE_UID, FieldOverride, Target, unit::Unit};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DatasourceKind {
    Prometheus,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    Dash,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DrawStyle {
    Line,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatcherKind {
    ByName,
}

#[derive(Clone, Copy, Serialize)]
pub enum PropertyId {
    #[serde(rename = "color")]
    Color,
    #[serde(rename = "custom.lineStyle")]
    LineStyle,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Calc {
    LastNotNull,
    Last,
    Max,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatColorMode {
    Value,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphMode {
    Area,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LegendDisplay {
    Table,
    List,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    Bottom,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TooltipMode {
    Single,
    Multi,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Desc,
}

// Reused by the `input` model, hence `Deserialize`.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelType {
    Stat,
    Timeseries,
}

// Optional timeseries styling vocabularies. Each derives `PartialEq` so the
// public setters can panic when handed the Grafana default (see `styling`), and
// `Deserialize` because the `input` model reuses them.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineInterpolation {
    Linear,
    Smooth,
    StepBefore,
    StepAfter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShowPoints {
    Auto,
    Never,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientMode {
    None,
    Opacity,
    Hue,
    Scheme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackingMode {
    None,
    Normal,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AxisPlacement {
    Auto,
    Left,
    Right,
    Hidden,
}

#[derive(Serialize)]
pub struct Stacking {
    pub mode: StackingMode,
    pub group: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Custom {
    pub draw_style: DrawStyle,
    pub line_width: u32,
    pub fill_opacity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_nulls: Option<bool>,
    // Optional styling — omitted (left at Grafana's default) unless a setter
    // fills it in. See `styling`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_interpolation: Option<LineInterpolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_points: Option<ShowPoints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_mode: Option<GradientMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stacking: Option<Stacking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_placement: Option<AxisPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_label: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Datasource {
    #[serde(rename = "type")]
    pub kind: DatasourceKind,
    pub uid: String,
}

impl Datasource {
    pub fn prometheus() -> Self {
        Self {
            kind: DatasourceKind::Prometheus,
            uid: DATASOURCE_UID.to_owned(),
        }
    }
}

// Reused by the `input` model, hence `Deserialize`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "mode")]
pub enum Color {
    Fixed {
        #[serde(rename = "fixedColor")]
        fixed_color: String,
    },
    PaletteClassic,
}

impl Color {
    pub fn fixed(color: impl Into<String>) -> Self {
        Self::Fixed {
            fixed_color: color.into(),
        }
    }

    #[must_use]
    pub const fn palette_classic() -> Self {
        Self::PaletteClassic
    }
}

#[derive(Serialize)]
pub struct LineStyle {
    pub dash: [u32; 2],
    pub fill: Fill,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum PropertyValue {
    Color(Color),
    LineStyle(LineStyle),
}

#[derive(Serialize)]
pub struct OverrideProperty {
    pub id: PropertyId,
    pub value: PropertyValue,
}

#[derive(Serialize)]
pub struct Matcher {
    pub id: MatcherKind,
    pub options: String,
}

#[derive(Serialize)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<Custom>,
    pub unit: Unit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u32>,
}

#[derive(Serialize)]
pub struct FieldConfig {
    pub defaults: Defaults,
    pub overrides: Vec<FieldOverride>,
}

#[derive(Serialize)]
pub struct ReduceOptions {
    pub calcs: Vec<Calc>,
    // Empty string means "all fields"; genuinely free-form, not a vocabulary.
    pub fields: String,
    pub values: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatOptions {
    pub color_mode: StatColorMode,
    pub graph_mode: GraphMode,
    pub reduce_options: ReduceOptions,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Legend {
    pub display_mode: LegendDisplay,
    pub placement: Placement,
    pub calcs: Vec<Calc>,
}

#[derive(Serialize)]
pub struct Tooltip {
    pub mode: TooltipMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortOrder>,
}

#[derive(Serialize)]
pub struct TimeSeriesOptions {
    pub legend: Legend,
    pub tooltip: Tooltip,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Options {
    Stat(StatOptions),
    TimeSeries(TimeSeriesOptions),
}

#[derive(Clone, Copy, Serialize)]
pub struct GridPos {
    pub h: u32,
    pub w: u32,
    pub x: u32,
    pub y: u32,
}

/// A fully positioned panel, ready to serialize.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelModel {
    pub datasource: Datasource,
    pub field_config: FieldConfig,
    pub grid_pos: GridPos,
    pub id: u32,
    pub options: Options,
    pub targets: Vec<Target>,
    pub title: String,
    #[serde(rename = "type")]
    pub panel_type: PanelType,
}

#[expect(
    clippy::trailing_empty_array,
    reason = "Grafana expects `list: []` for the blocks we don't populate"
)]
#[derive(Serialize, Default)]
pub struct EmptyList {
    pub list: [u8; 0],
}

#[derive(Serialize)]
pub struct TimeRange {
    // Free-form Grafana time expressions, not a closed vocabulary.
    pub from: String,
    pub to: String,
}
