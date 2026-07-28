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
    DEFAULT_FILL_OPACITY, Unit,
    input::{Defaults, PanelInput},
    schema::{
        AxisPlacement, Color, GradientMode, LineInterpolation, PanelType, ShowPoints, StackingMode,
        ThresholdMode, Thresholds,
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
        PanelType::Gauge => format!("Panel::gauge({:?})", panel.title),
    };
    write!(expr, "\n    .width({})", panel.grid_pos.w)?;

    let defaults = &panel.field_config.defaults;
    write_defaults(&mut expr, defaults)?;

    if let Some(custom) = &defaults.custom {
        // Emitted against the builder's default, not Grafana's (which is 0), so
        // a genuinely unfilled panel still round-trips.
        if let Some(opacity) = custom.fill_opacity.filter(|&o| o != DEFAULT_FILL_OPACITY) {
            write!(expr, "\n    .fill_opacity({opacity})")?;
        }
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

/// The field-level setters — everything outside the `custom` styling block.
fn write_defaults(expr: &mut String, defaults: &Defaults) -> Result<(), std::fmt::Error> {
    // `short` is the builder's own default unit, so it round-trips without a call.
    if let Some(unit) = defaults.unit.as_deref().filter(|u| *u != "short") {
        write!(
            expr,
            "\n    .unit({})",
            Unit::from_id(unit).to_rust_source()
        )?;
    }
    if let Some(decimals) = defaults.decimals {
        write!(expr, "\n    .decimals({decimals})")?;
    }
    // Only a fixed color is worth emitting; `palette-classic` is Grafana's default.
    if let Some(Color::Fixed { fixed_color }) = &defaults.color {
        write!(expr, "\n    .color(Color::fixed({fixed_color:?}))")?;
    }
    if let Some(min) = defaults.min {
        write!(expr, "\n    .min({min:?})")?;
    }
    if let Some(max) = defaults.max {
        write!(expr, "\n    .max({max:?})")?;
    }

    let ladder = defaults
        .thresholds
        .as_ref()
        .filter(|thresholds| is_expressible_ladder(thresholds))
        .and_then(|thresholds| thresholds.steps.split_first());
    if let Some((base, steps)) = ladder {
        write!(expr, "\n    .thresholds(Thresholds::base({:?})", base.color)?;
        for step in steps {
            // A non-base step without a value is nonsense Grafana wouldn't render.
            if let Some(value) = step.value {
                write!(expr, ".step({value:?}, {:?})", step.color)?;
            }
        }
        expr.push(')');
    }

    Ok(())
}

/// Whether a ladder is worth emitting: `percentage` mode is beyond the builder,
/// and green/red-at-80 is the ladder Grafana attaches to every panel by default.
fn is_expressible_ladder(thresholds: &Thresholds) -> bool {
    if thresholds.mode != ThresholdMode::Absolute {
        return false;
    }
    !matches!(
        thresholds.steps.as_slice(),
        [base, red]
            if base.color == "green"
                && base.value.is_none()
                && red.color == "red"
                && red.value == Some(80.0)
    )
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
