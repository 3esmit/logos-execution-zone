//! Reverse of the builder: turn a single parsed panel back into the Rust
//! `Panel::…` builder expression, omitting values Grafana supplies by default.
//! Backs the `panel_json_to_rust` binary.
//!
//! Input is parsed leniently (see [`crate::input`]) so a raw Grafana panel
//! export — full of fields and vocabularies we don't model — still works;
//! anything unrecognized is dropped. The emitted expression is valid but only
//! lightly formatted, so `cargo fmt` re-indents it once it lands in a file.

use std::fmt::Write as _;

use crate::{
    input::PanelInput,
    schema::{
        AxisPlacement, Color, GradientMode, LineInterpolation, PanelType, ShowPoints, StackingMode,
    },
};

/// Parse one Grafana panel JSON (Inspect → Panel JSON) and emit the Rust
/// builder expression that reproduces it (minus Grafana defaults).
pub fn panel_to_rust_source(json: &str) -> serde_json::Result<String> {
    let panel: PanelInput = serde_json::from_str(json)?;
    Ok(format!("{}\n", panel_expr(&panel)))
}

/// A `Panel::…()` expression with one method call per line.
fn panel_expr(panel: &PanelInput) -> String {
    panel_expr_inner(panel).expect("writing to a String never fails")
}

fn panel_expr_inner(panel: &PanelInput) -> Result<String, std::fmt::Error> {
    let mut expr = match panel.panel_type {
        PanelType::Stat => format!("Panel::stat({:?})", panel.title),
        PanelType::Timeseries => format!("Panel::timeseries({:?})", panel.title),
    };
    write!(expr, "\n    .width({})", panel.grid_pos.w)?;

    let defaults = &panel.field_config.defaults;
    // `short` is the builder's own default unit, so it round-trips without a call.
    if let Some(unit) = defaults.unit.as_deref().filter(|u| *u != "short") {
        write!(expr, "\n    .unit({unit:?})")?;
    }
    if let Some(decimals) = defaults.decimals {
        write!(expr, "\n    .decimals({decimals})")?;
    }
    // Only a fixed color is worth emitting; `palette-classic` is Grafana's default.
    if let Some(Color::Fixed { fixed_color }) = &defaults.color {
        write!(expr, "\n    .color(Color::fixed({fixed_color:?}))")?;
    }

    if let Some(custom) = &defaults.custom {
        if custom.span_nulls == Some(true) {
            expr.push_str("\n    .span_nulls()");
        }
        // Each optional styling field is emitted only when it differs from the
        // Grafana default (matching the setters' panic-on-default contract).
        if let Some(value) = custom
            .line_interpolation
            .filter(|&v| v != LineInterpolation::Linear)
        {
            write!(
                expr,
                "\n    .line_interpolation(LineInterpolation::{})",
                line_interp(value)
            )?;
        }
        if let Some(value) = custom.show_points.filter(|&v| v != ShowPoints::Auto) {
            write!(
                expr,
                "\n    .show_points(ShowPoints::{})",
                show_points(value)
            )?;
        }
        if let Some(value) = custom.gradient_mode.filter(|&v| v != GradientMode::None) {
            write!(
                expr,
                "\n    .gradient_mode(GradientMode::{})",
                gradient_mode(value)
            )?;
        }
        let stacking = custom.stacking.as_ref().and_then(|s| s.mode);
        if let Some(mode) = stacking.filter(|&m| m != StackingMode::None) {
            write!(
                expr,
                "\n    .stacking(StackingMode::{})",
                stacking_mode(mode)
            )?;
        }
        if let Some(value) = custom.axis_placement.filter(|&v| v != AxisPlacement::Auto) {
            write!(
                expr,
                "\n    .axis_placement(AxisPlacement::{})",
                axis_placement(value)
            )?;
        }
        if let Some(label) = custom.axis_label.as_deref().filter(|l| !l.is_empty()) {
            write!(expr, "\n    .axis_label({label:?})")?;
        }
    }

    for over in &panel.field_config.overrides {
        // Only `byName` matchers map to the builder; skip anything else.
        let Some(name) = over.matcher.by_name() else {
            continue;
        };
        let mut calls = String::new();
        for property in &over.properties {
            match property.id.as_str() {
                "color" => {
                    if let Ok(Color::Fixed { fixed_color }) =
                        serde_json::from_value::<Color>(property.value.clone())
                    {
                        write!(calls, ".color(Color::fixed({fixed_color:?}))")?;
                    }
                }
                "custom.lineStyle" => calls.push_str(".dashed_line()"),
                _ => {} // property kind the builder can't express — drop it
            }
        }
        // An override with nothing representable adds no information.
        if !calls.is_empty() {
            write!(
                expr,
                "\n    .with_override(FieldOverride::by_name({name:?}){calls})"
            )?;
        }
    }

    for target in &panel.targets {
        if target.expr.is_empty() {
            continue;
        }
        write!(expr, "\n    .target(Target::new({:?})", target.expr)?;
        // `__auto` is Grafana's "no explicit legend" sentinel, i.e. the default.
        if !target.legend.is_empty() && target.legend != "__auto" {
            write!(expr, ".legend({:?})", target.legend)?;
        }
        expr.push(')');
    }

    Ok(expr)
}

const fn line_interp(value: LineInterpolation) -> &'static str {
    match value {
        LineInterpolation::Linear => "Linear",
        LineInterpolation::Smooth => "Smooth",
        LineInterpolation::StepBefore => "StepBefore",
        LineInterpolation::StepAfter => "StepAfter",
    }
}

const fn show_points(value: ShowPoints) -> &'static str {
    match value {
        ShowPoints::Auto => "Auto",
        ShowPoints::Never => "Never",
        ShowPoints::Always => "Always",
    }
}

const fn gradient_mode(value: GradientMode) -> &'static str {
    match value {
        GradientMode::None => "None",
        GradientMode::Opacity => "Opacity",
        GradientMode::Hue => "Hue",
        GradientMode::Scheme => "Scheme",
    }
}

const fn stacking_mode(value: StackingMode) -> &'static str {
    match value {
        StackingMode::None => "None",
        StackingMode::Normal => "Normal",
        StackingMode::Percent => "Percent",
    }
}

const fn axis_placement(value: AxisPlacement) -> &'static str {
    match value {
        AxisPlacement::Auto => "Auto",
        AxisPlacement::Left => "Left",
        AxisPlacement::Right => "Right",
        AxisPlacement::Hidden => "Hidden",
    }
}
