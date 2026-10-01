//! The Monitors model's rows, built from the outputs the compositor has:
//! `tests::a_monitor_row_carries_its_whole_and_work_areas`.

use std::collections::BTreeMap;

use smithay::utils::{Logical, Rectangle};

use super::diff::Row;
use crate::json::Json;
use crate::state::Solium;

/// One row per monitor, in the order the compositor holds them: its name, its
/// whole and work areas in the global space, its scale, its transform, and
/// whether it is the primary one.
/// `tests::a_monitor_row_carries_its_whole_and_work_areas`.
pub(crate) fn rows(state: &Solium) -> Vec<Row> {
    let primary = state.primary_output();
    state
        .space
        .outputs()
        .filter_map(|output| {
            let whole = state.space.output_geometry(output)?;
            let area = state.work_area_on(output).unwrap_or(whole);
            Some(Row {
                key: output.name(),
                values: BTreeMap::from([
                    ("name", Json::Text(output.name())),
                    ("whole", rect(whole)),
                    ("area", rect(area)),
                    (
                        "scale",
                        Json::Number(output.current_scale().fractional_scale()),
                    ),
                    (
                        "transform",
                        Json::Text(format!("{:?}", output.current_transform()).to_lowercase()),
                    ),
                    ("primary", Json::Bool(primary.as_ref() == Some(output))),
                ]),
            })
        })
        .collect()
}

/// A rectangle as QML's `rect` reads it: `x`, `y`, `width` and `height`.
/// `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
pub(crate) fn rect(rect: Rectangle<i32, Logical>) -> Json {
    Json::Object(BTreeMap::from([
        ("x".to_owned(), Json::Number(f64::from(rect.loc.x))),
        ("y".to_owned(), Json::Number(f64::from(rect.loc.y))),
        ("width".to_owned(), Json::Number(f64::from(rect.size.w))),
        ("height".to_owned(), Json::Number(f64::from(rect.size.h))),
    ]))
}

#[cfg(test)]
mod tests {
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
    use smithay::reexports::wayland_server::Display;

    use crate::json::Json;
    use crate::state::Solium;

    /// **A monitor's row is its name, its whole and work areas in the global
    /// space, its scale, its transform and whether it is primary.**
    #[test]
    fn a_monitor_row_carries_its_whole_and_work_areas() {
        let display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        let output = Output::new(
            "rows-1".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "rows".to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output, (1920, 0));

        let rows = super::rows(&state);
        let [row] = rows.as_slice() else {
            panic!("one monitor, one row: {rows:?}")
        };
        assert_eq!(row.key, "rows-1");
        let whole = super::rect(smithay::utils::Rectangle::new(
            (1920, 0).into(),
            (1920, 1080).into(),
        ));
        assert_eq!(row.values.get("whole"), Some(&whole));
        assert_eq!(
            row.values.get("area"),
            Some(&whole),
            "no bar and no reserve: the work area is the monitor"
        );
        assert_eq!(row.values.get("scale"), Some(&Json::Number(1.0)));
        assert_eq!(
            row.values.get("transform"),
            Some(&Json::Text("normal".to_owned()))
        );
        assert_eq!(
            row.values.get("primary"),
            Some(&Json::Bool(true)),
            "the only monitor is the primary one"
        );
    }

    /// **A turned monitor's `transform` is Smithay's name in lower case**, as
    /// `sol.monitors()` spells it, not as `sol.monitors{ ... }` takes it.
    #[test]
    fn a_turned_monitor_row_names_its_transform_as_smithay_does() {
        use smithay::utils::Transform;

        let display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        let output = Output::new(
            "rows-turned-1".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "rows".to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            Some(Transform::_90),
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output, (0, 0));
        let transform = |state: &Solium| {
            super::rows(state)
                .first()
                .and_then(|row| row.values.get("transform").cloned())
        };
        assert_eq!(transform(&state), Some(Json::Text("_90".to_owned())));

        output.change_current_state(None, Some(Transform::Flipped270), None, None);
        assert_eq!(transform(&state), Some(Json::Text("flipped270".to_owned())));
    }
}
