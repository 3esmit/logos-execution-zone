//! The serializable Grafana dashboard schema — the internal data model the
//! public builders assemble into.

use serde::Serialize;

use crate::{DATASOURCE_UID, FieldOverride, Target};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DatasourceKind {
    Prometheus,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Fixed,
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

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelType {
    Stat,
    Timeseries,
}

#[derive(Clone, Serialize)]
pub struct Datasource {
    #[serde(rename = "type")]
    pub kind: DatasourceKind,
    pub uid: &'static str,
}

impl Datasource {
    pub fn prometheus() -> Self {
        Self {
            kind: DatasourceKind::Prometheus,
            uid: DATASOURCE_UID,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Color {
    pub mode: ColorMode,
    pub fixed_color: String,
}

impl Color {
    pub fn fixed(color: String) -> Self {
        Self {
            mode: ColorMode::Fixed,
            fixed_color: color,
        }
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
#[serde(rename_all = "camelCase")]
pub struct Custom {
    pub draw_style: DrawStyle,
    pub line_width: u32,
    pub fill_opacity: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_nulls: Option<bool>,
}

#[derive(Serialize)]
pub struct Defaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<Custom>,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub fields: &'static str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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

#[derive(Serialize, Default)]
pub struct EmptyList {
    pub list: [u8; 0],
}

#[derive(Serialize)]
pub struct TimeRange {
    // Free-form Grafana time expressions, not a closed vocabulary.
    pub from: &'static str,
    pub to: &'static str,
}
