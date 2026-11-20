//! Every group's own pose, for `Solium.groups`: what a window manager's
//! `present-group` has each group at, this pass, by its name. A scene that
//! draws a world in three dimensions, such as a room in Qt Quick 3D, puts its
//! camera where the window manager's camera groups are, and the windows
//! those groups carry are drawn where the scene's walls are, because both are
//! read at the one instant the pass is drawn at (`render::frame_tick`).
//! Sent when it changed, which is every frame a group moves and none after.
//! `tests::a_groups_pose_is_its_numbers_at_the_instant_asked`,
//! `qml::hosted::tests::a_published_group_reaches_solium_groups`.

use std::{collections::BTreeMap, time::Duration};

use crate::{group::Groups, json::Json};

/// Each group's own shift at `now` as one JSON object, by name: the numbers
/// of its pose (`x`, `y`, `z`, `rotateX`, `rotateY`, `rotateZ`, `scale`,
/// `originX`, `originY`, `originZ`, `perspective`), its displacement (`dx`,
/// `dy`) and its `opacity`, the names a window manager writes them by.
pub(crate) fn json(groups: &Groups, now: Duration) -> String {
    let each = groups
        .poses(now)
        .map(|(name, shift)| {
            let pose = shift.pose;
            let numbers = [
                ("x", pose.translate[0]),
                ("y", pose.translate[1]),
                ("z", pose.translate[2]),
                ("rotateX", pose.rotate[0]),
                ("rotateY", pose.rotate[1]),
                ("rotateZ", pose.rotate[2]),
                ("scale", pose.scale),
                ("originX", pose.origin[0]),
                ("originY", pose.origin[1]),
                ("originZ", pose.origin[2]),
                ("perspective", pose.perspective),
                ("dx", shift.dx),
                ("dy", shift.dy),
                ("opacity", f64::from(shift.opacity)),
            ];
            let object: BTreeMap<String, Json> = numbers
                .into_iter()
                .map(|(key, value)| (key.to_owned(), Json::Number(value)))
                .collect();
            (name.to_owned(), Json::Object(object))
        })
        .collect();
    Json::Object(each).render()
}

/// Send the groups' poses at `now` if they differ from what Qt last took.
/// A batch Qt could not take, before it has started, is sent again.
pub(crate) fn publish(groups: &Groups, now: Duration, last: &mut Option<String>) {
    let next = json(groups, now);
    if last.as_deref() == Some(next.as_str()) {
        return;
    }
    if crate::qml::hosted::set_groups(&next) {
        *last = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use solium_animation::Curve;

    use crate::group::{Groups, Pose, Selection, Shift};
    use crate::json::Json;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// **A group's pose is its numbers at the instant asked**: a camera
    /// turning from 0 to 90 degrees over a second reads 45 half way, by the
    /// name the window manager wrote, with its origin and perspective; a
    /// group never presented reads at rest.
    #[test]
    fn a_groups_pose_is_its_numbers_at_the_instant_asked() {
        let mut groups = Groups::default();
        groups.declare("turn", Selection::default(), None, at(0));
        groups.declare("still", Selection::default(), None, at(0));
        let pose = |degrees: f64| {
            Shift::NONE.posed(Pose {
                rotate: [0.0, degrees, 0.0],
                origin: [800.0, 450.0, 960.0],
                perspective: 960.0,
                ..Pose::NONE
            })
        };
        groups.present("turn", pose(0.0), at(0), Duration::ZERO, Curve::Linear);
        groups.present("turn", pose(90.0), at(0), at(1000), Curve::Linear);
        let read = |ms: u64| match Json::parse(&super::json(&groups, at(ms))) {
            Some(Json::Object(each)) => each,
            other => panic!("not an object: {other:?}"),
        };
        let number =
            |each: &std::collections::BTreeMap<String, Json>, group: &str, key: &str| match each
                .get(group)
            {
                Some(Json::Object(numbers)) => match numbers.get(key) {
                    Some(Json::Number(value)) => *value,
                    other => panic!("{group}.{key} is {other:?}"),
                },
                other => panic!("{group} is {other:?}"),
            };
        let half = read(500);
        assert!((number(&half, "turn", "rotateY") - 45.0).abs() < 1e-9);
        assert!((number(&half, "turn", "originZ") - 960.0).abs() < 1e-9);
        assert!((number(&half, "turn", "perspective") - 960.0).abs() < 1e-9);
        assert!((number(&read(1000), "turn", "rotateY") - 90.0).abs() < 1e-9);
        assert!((number(&half, "still", "scale") - 1.0).abs() < 1e-9);
        assert!((number(&half, "still", "opacity") - 1.0).abs() < 1e-9);
    }
}
