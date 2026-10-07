//! The Monitors model's rows, built from the outputs the compositor has:
//! `tests::a_monitor_row_carries_its_whole_and_work_areas`.

use std::collections::BTreeMap;

use smithay::utils::{Logical, Rectangle};

use super::diff::Row;
use crate::json::Json;
use crate::state::Solium;

/// One row per monitor, in the order the compositor holds them: its name, its
/// whole and work areas in the global space, its scale, its transform,
/// whether it is the primary one, what is reserved on each edge, whether it
/// is on, whether the pointer is on it and whether it is the active one.
/// `tests::a_monitor_row_carries_its_whole_and_work_areas`,
/// `tests::a_monitor_row_says_what_is_reserved_and_where_the_pointer_is`.
pub(crate) fn rows(state: &Solium) -> Vec<Row> {
    let primary = state.primary_output();
    let active = state.active_output();
    let pointer = state
        .seat
        .get_pointer()
        .map(|pointer| pointer.current_location());
    state
        .space
        .outputs()
        .filter_map(|output| {
            let whole = state.space.output_geometry(output)?;
            let area = state.work_area_on(output).unwrap_or(whole);
            let power = if state.power.is_off(output) {
                "off"
            } else {
                "on"
            };
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
                    ("reserved", reserved(whole, area)),
                    ("power", Json::Text(power.to_owned())),
                    (
                        "pointer",
                        Json::Bool(pointer.is_some_and(|at| whole.to_f64().contains(at))),
                    ),
                    ("active", Json::Bool(active.as_ref() == Some(output))),
                ]),
            })
        })
        .collect()
}

/// What the work area lost on each edge: layer-shell zones and hosted
/// reserves together.
/// `tests::a_monitor_row_says_what_is_reserved_and_where_the_pointer_is`.
fn reserved(whole: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> Json {
    let edge = |value: i32| Json::Number(f64::from(value));
    Json::Object(BTreeMap::from([
        ("top".to_owned(), edge(area.loc.y - whole.loc.y)),
        ("left".to_owned(), edge(area.loc.x - whole.loc.x)),
        (
            "bottom".to_owned(),
            edge((whole.loc.y + whole.size.h) - (area.loc.y + area.size.h)),
        ),
        (
            "right".to_owned(),
            edge((whole.loc.x + whole.size.w) - (area.loc.x + area.size.w)),
        ),
    ]))
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

    /// **A row says what is reserved on each edge, whether it is on, and
    /// whether the pointer is on it.**
    #[test]
    fn a_monitor_row_says_what_is_reserved_and_where_the_pointer_is() {
        let display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        let output = Output::new(
            "rows-reserved".to_owned(),
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
        state.space.map_output(&output, (0, 0));
        let mut declared = crate::scripted::Declaration::for_test(
            "bar",
            std::path::PathBuf::from("/nonexistent/rows.qml"),
            crate::scripted::Layer::Top,
            crate::scripted::On::EveryMonitor,
        );
        declared.reserve = crate::scripted::Edges {
            bottom: 48,
            ..Default::default()
        };
        state.declare_surface(declared);

        let rows = super::rows(&state);
        let [row] = rows.as_slice() else {
            panic!("one monitor, one row: {rows:?}")
        };
        let reserved = row.values.get("reserved").expect("a reserved role");
        assert_eq!(reserved.get("bottom"), Some(&Json::Number(48.0)));
        assert_eq!(reserved.get("top"), Some(&Json::Number(0.0)));
        assert_eq!(row.values.get("power"), Some(&Json::Text("on".to_owned())));
        assert_eq!(
            row.values.get("pointer"),
            Some(&Json::Bool(true)),
            "the pointer starts at 0,0, on this monitor"
        );
        assert_eq!(row.values.get("active"), Some(&Json::Bool(true)));
    }

    /// **On two monitors, only the one the pointer is on says so and is the
    /// active one, each edge says what was reserved on it, and a monitor
    /// turned off says `"off"` while the other stays `"on"`.**
    #[test]
    fn on_two_monitors_only_the_pointers_is_active_and_each_says_its_own_edges() {
        let display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        let monitor = |name: &str| {
            let output = Output::new(
                name.to_owned(),
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
            output
        };
        let (left, right) = (monitor("rows-two-left"), monitor("rows-two-right"));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (1920, 0));
        let mut declared = crate::scripted::Declaration::for_test(
            "dock",
            std::path::PathBuf::from("/nonexistent/rows-two.qml"),
            crate::scripted::Layer::Top,
            crate::scripted::On::Monitor("rows-two-right".to_owned()),
        );
        declared.reserve = crate::scripted::Edges {
            left: 30,
            right: 20,
            ..Default::default()
        };
        state.declare_surface(declared);
        state
            .seat
            .get_pointer()
            .expect("a pointer")
            .set_location((2000.0, 500.0).into());
        assert!(state.set_power(&left, false), "the left monitor turns off");

        let rows = super::rows(&state);
        let values = |name: &str| {
            rows.iter()
                .find(|row| row.key == name)
                .map(|row| row.values.clone())
                .expect("a row for each monitor")
        };
        let (on_left, on_right) = (values("rows-two-left"), values("rows-two-right"));
        let role = |values: &std::collections::BTreeMap<&'static str, Json>, name: &str| {
            values.get(name).cloned().unwrap_or(Json::Null)
        };
        assert_eq!(
            (
                role(&on_left, "pointer"),
                role(&on_left, "active"),
                role(&on_left, "power"),
                role(&on_right, "pointer"),
                role(&on_right, "active"),
                role(&on_right, "power"),
            ),
            (
                Json::Bool(false),
                Json::Bool(false),
                Json::Text("off".to_owned()),
                Json::Bool(true),
                Json::Bool(true),
                Json::Text("on".to_owned()),
            ),
            "(left: pointer, active, power; right: pointer, active, power)"
        );
        let edges = |values: &std::collections::BTreeMap<&'static str, Json>| {
            let reserved = role(values, "reserved");
            ["top", "right", "bottom", "left"].map(|edge| reserved.get(edge).cloned())
        };
        assert_eq!(
            (edges(&on_left), edges(&on_right)),
            (
                [
                    Some(Json::Number(0.0)),
                    Some(Json::Number(0.0)),
                    Some(Json::Number(0.0)),
                    Some(Json::Number(0.0)),
                ],
                [
                    Some(Json::Number(0.0)),
                    Some(Json::Number(20.0)),
                    Some(Json::Number(0.0)),
                    Some(Json::Number(30.0)),
                ],
            ),
            "(the left monitor's top, right, bottom and left, then the right one's)"
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
