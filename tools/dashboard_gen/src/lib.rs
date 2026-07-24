//! A tiny, hand-rolled Grafana dashboard builder.
//!
//! This is a deliberately small subset of what the (Rust-less) Grafana
//! Foundation SDK does: model only the panel types and options we actually use,
//! expose a fluent builder, and render to the same dashboard JSON Grafana
//! provisions. The payoff over hand-written JSON:
//!
//! * metric names live in Rust `const`s, so a rename is a compile error here;
//! * repetitive structure (percentile targets, grid layout, legend/tooltip defaults) collapses into
//!   a single call instead of copy-pasted JSON.
//!
//! Build a [`Dashboard`] and serialize it directly.

use schema::{
    Calc, Color, Custom, Datasource, Defaults, DrawStyle, EmptyList, FieldConfig, Fill, GraphMode,
    GridPos, Legend, LegendDisplay, LineStyle, Matcher, MatcherKind, Options, OverrideProperty,
    PanelModel, PanelType, Placement, PropertyId, PropertyValue, ReduceOptions, SortOrder,
    StatColorMode, StatOptions, TimeRange, TimeSeriesOptions, Tooltip, TooltipMode,
};
use serde::Serialize;

mod schema;

/// Datasource uid every panel/target points at. Dashboards stay portable across
/// environments because they reference the datasource by this stable uid rather
/// than by a per-environment URL.
pub const DATASOURCE_UID: &str = "prometheus";

fn default_unit() -> String {
    "short".to_owned()
}

fn ref_letter(index: usize) -> String {
    ((b'A' + index as u8) as char).to_string()
}

/// A single Prometheus query within a panel.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    datasource: Datasource,
    expr: String,
    #[serde(rename = "legendFormat")]
    legend: String,
    ref_id: String,
}

impl Target {
    pub fn new(expr: impl Into<String>) -> Self {
        Self {
            datasource: Datasource::prometheus(),
            expr: expr.into(),
            legend: String::new(),
            ref_id: ref_letter(0),
        }
    }

    pub fn legend(mut self, legend: impl Into<String>) -> Self {
        self.legend = legend.into();
        self
    }
}

/// A per-series style override, matched by series name.
#[derive(Serialize)]
pub struct FieldOverride {
    matcher: Matcher,
    properties: Vec<OverrideProperty>,
}

impl FieldOverride {
    pub fn by_name(name: impl Into<String>) -> Self {
        Self {
            matcher: Matcher {
                id: MatcherKind::ByName,
                options: name.into(),
            },
            properties: Vec::new(),
        }
    }

    pub fn fixed_color(mut self, color: impl Into<String>) -> Self {
        self.properties.push(OverrideProperty {
            id: PropertyId::Color,
            value: PropertyValue::Color(Color::fixed(color.into())),
        });
        self
    }

    pub fn dashed_line(mut self) -> Self {
        self.properties.push(OverrideProperty {
            id: PropertyId::LineStyle,
            value: PropertyValue::LineStyle(LineStyle {
                dash: [8, 4],
                fill: Fill::Dash,
            }),
        });
        self
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Stat,
    TimeSeries,
}

/// A dashboard panel builder. Grid position and panel id are assigned by
/// [`Dashboard::row`]; everything else is set here.
pub struct Panel {
    title: String,
    kind: Kind,
    targets: Vec<Target>,
    width: u32,
    unit: Option<String>,
    decimals: Option<u32>,
    fixed_color: Option<String>,
    span_nulls: bool,
    overrides: Vec<FieldOverride>,
}

impl Panel {
    fn new(title: impl Into<String>, kind: Kind) -> Self {
        Self {
            title: title.into(),
            kind,
            targets: Vec::new(),
            width: 0,
            unit: None,
            decimals: None,
            fixed_color: None,
            span_nulls: false,
            overrides: Vec::new(),
        }
    }

    /// A single big-number panel.
    pub fn stat(title: impl Into<String>) -> Self {
        Self::new(title, Kind::Stat)
    }

    /// A time-series line panel.
    pub fn timeseries(title: impl Into<String>) -> Self {
        Self::new(title, Kind::TimeSeries)
    }

    /// Grid width in Grafana's 24-column units. Unset panels split the row's
    /// remaining width evenly.
    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub fn decimals(mut self, decimals: u32) -> Self {
        self.decimals = Some(decimals);
        self
    }

    pub fn fixed_color(mut self, color: impl Into<String>) -> Self {
        self.fixed_color = Some(color.into());
        self
    }

    pub fn span_nulls(mut self) -> Self {
        self.span_nulls = true;
        self
    }

    pub fn target(mut self, target: Target) -> Self {
        self.targets.push(target);
        self
    }

    pub fn targets(mut self, targets: impl IntoIterator<Item = Target>) -> Self {
        self.targets.extend(targets);
        self
    }

    pub fn with_override(mut self, over: FieldOverride) -> Self {
        self.overrides.push(over);
        self
    }

    fn finalize(self, id: u32, grid_pos: GridPos) -> PanelModel {
        let targets: Vec<Target> = self
            .targets
            .into_iter()
            .enumerate()
            .map(|(i, mut t)| {
                t.ref_id = ref_letter(i);
                t
            })
            .collect();

        let unit = self.unit.unwrap_or_else(default_unit);
        let (defaults, options, panel_type) = match self.kind {
            Kind::Stat => {
                let defaults = Defaults {
                    color: self.fixed_color.map(Color::fixed),
                    custom: None,
                    unit,
                    decimals: self.decimals,
                };
                let options = Options::Stat(StatOptions {
                    color_mode: StatColorMode::Value,
                    graph_mode: GraphMode::Area,
                    reduce_options: ReduceOptions {
                        calcs: vec![Calc::LastNotNull],
                        fields: "",
                        values: false,
                    },
                });
                (defaults, options, PanelType::Stat)
            }
            Kind::TimeSeries => {
                let defaults = Defaults {
                    color: None,
                    custom: Some(Custom {
                        draw_style: DrawStyle::Line,
                        line_width: 1,
                        fill_opacity: 10,
                        span_nulls: self.span_nulls.then_some(true),
                    }),
                    unit,
                    decimals: None,
                };
                // Panels with several series read better as a sortable table with
                // a multi-series tooltip; single-series panels stay compact.
                let multi = targets.len() > 1;
                let options = Options::TimeSeries(TimeSeriesOptions {
                    legend: Legend {
                        display_mode: if multi {
                            LegendDisplay::Table
                        } else {
                            LegendDisplay::List
                        },
                        placement: Placement::Bottom,
                        calcs: vec![Calc::Last, Calc::Max],
                    },
                    tooltip: if multi {
                        Tooltip {
                            mode: TooltipMode::Multi,
                            sort: Some(SortOrder::Desc),
                        }
                    } else {
                        Tooltip {
                            mode: TooltipMode::Single,
                            sort: None,
                        }
                    },
                });
                (defaults, options, PanelType::Timeseries)
            }
        };

        PanelModel {
            datasource: Datasource::prometheus(),
            field_config: FieldConfig {
                defaults,
                overrides: self.overrides,
            },
            grid_pos,
            id,
            options,
            targets,
            title: self.title,
            panel_type,
        }
    }
}

/// A dashboard, built row by row. Serialize it directly to get the JSON.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    annotations: EmptyList,
    editable: bool,
    graph_tooltip: u32,
    panels: Vec<PanelModel>,
    refresh: String,
    schema_version: u32,
    tags: Vec<String>,
    templating: EmptyList,
    time: TimeRange,
    timezone: String,
    title: String,
    uid: String,

    // Layout cursor — not part of the dashboard schema.
    #[serde(skip)]
    next_id: u32,
    #[serde(skip)]
    cursor_y: u32,
}

impl Dashboard {
    pub fn new(title: impl Into<String>, uid: impl Into<String>) -> Self {
        Self {
            annotations: EmptyList::default(),
            editable: true,
            graph_tooltip: 1,
            panels: Vec::new(),
            refresh: "5s".to_owned(),
            schema_version: 39,
            tags: Vec::new(),
            templating: EmptyList::default(),
            time: TimeRange {
                from: "now-15m",
                to: "now",
            },
            timezone: String::new(),
            title: title.into(),
            uid: uid.into(),
            next_id: 1,
            cursor_y: 0,
        }
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn refresh(mut self, refresh: impl Into<String>) -> Self {
        self.refresh = refresh.into();
        self
    }

    /// Place a horizontal row of panels at the current vertical cursor. Panel
    /// ids, x offsets and y are assigned here; unset widths split the remaining
    /// 24 columns evenly.
    pub fn row(mut self, height: u32, panels: impl IntoIterator<Item = Panel>) -> Self {
        let panels: Vec<Panel> = panels.into_iter().collect();
        let specified: u32 = panels.iter().map(|p| p.width).sum();
        let auto_count = panels.iter().filter(|p| p.width == 0).count() as u32;
        let auto_width = if auto_count > 0 {
            24u32.saturating_sub(specified) / auto_count
        } else {
            0
        };

        let mut x = 0;
        for panel in panels {
            let w = if panel.width == 0 {
                auto_width
            } else {
                panel.width
            };
            let grid_pos = GridPos {
                h: height,
                w,
                x,
                y: self.cursor_y,
            };
            let id = self.next_id;
            self.next_id += 1;
            x += w;
            self.panels.push(panel.finalize(id, grid_pos));
        }
        self.cursor_y += height;
        self
    }
}

/// Percentile line targets for a summary metric: `p50`, `p90`, … each querying
/// the matching `quantile="0.x"` series.
pub fn percentiles(metric: &str, percentiles: &[u32]) -> Vec<Target> {
    percentiles_labeled(metric, percentiles, "")
}

/// Like [`percentiles`], but appends `legend_suffix` to every legend — handy
/// when the metric carries labels (e.g. ` · {{kind}} · {{origin}}`).
pub fn percentiles_labeled(metric: &str, percentiles: &[u32], legend_suffix: &str) -> Vec<Target> {
    percentiles
        .iter()
        .map(|&p| {
            let quantile = f64::from(p) / 100.0;
            Target::new(format!("{metric}{{quantile=\"{quantile}\"}}"))
                .legend(format!("p{p}{legend_suffix}"))
        })
        .collect()
}

/// An `avg` target for a summary metric: `rate(sum) / rate(count)` over 1m.
pub fn avg(metric: &str) -> Target {
    Target::new(format!("rate({metric}_sum[1m]) / rate({metric}_count[1m])")).legend("avg")
}

/// A per-minute rate target for a counter metric.
pub fn rate_per_min(metric: &str, legend: &str) -> Target {
    Target::new(format!("rate({metric}[1m]) * 60")).legend(legend)
}
