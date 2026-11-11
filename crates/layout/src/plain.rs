//! An arrangement as plain data: whole numbers, numbers and lists of them,
//! in no language, so a tree or a strip can be kept outside the program that
//! made it and read back whole ([`crate::tree::Tiling::to_plain`],
//! [`crate::scroller::Scroller::to_plain`]).
//!
//! Reading is strict. Data that is not an arrangement is refused, never
//! mended: a tree with a branch into nothing would otherwise be found out in
//! the middle of a layout.

use crate::{Floors, Minimum};

/// Plain data.
#[derive(Clone, Debug, PartialEq)]
pub enum Plain {
    /// A whole number: a window, a place in a list, a choice.
    Int(i64),
    Num(f64),
    List(Vec<Plain>),
}

impl Plain {
    /// A window's id, every bit of it.
    #[must_use]
    pub fn window(id: u64) -> Self {
        Self::Int(i64::from_ne_bytes(id.to_ne_bytes()))
    }

    /// A place in a list.
    #[must_use]
    pub fn place(at: usize) -> Self {
        Self::Int(i64::try_from(at).unwrap_or(i64::MAX))
    }

    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(whole) => Some(*whole),
            _ => None,
        }
    }

    /// A number that is one: not an infinity, and not `NaN`.
    #[must_use]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(number) if number.is_finite() => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[Self]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_window(&self) -> Option<u64> {
        self.as_int()
            .map(|whole| u64::from_ne_bytes(whole.to_ne_bytes()))
    }

    #[must_use]
    pub fn as_place(&self) -> Option<usize> {
        self.as_int().and_then(|whole| usize::try_from(whole).ok())
    }
}

/// Each window's floor, in the order of their ids, so the same floors are
/// the same data.
pub(crate) fn of_floors(floors: &Floors) -> Plain {
    let mut floors: Vec<(&u64, &Minimum)> = floors.iter().collect();
    floors.sort_by_key(|(id, _)| **id);
    Plain::List(
        floors
            .into_iter()
            .map(|(id, floor)| {
                Plain::List(vec![
                    Plain::window(*id),
                    Plain::Num(floor.w),
                    Plain::Num(floor.h),
                ])
            })
            .collect(),
    )
}

pub(crate) fn floors(plain: &Plain) -> Option<Floors> {
    plain
        .as_list()?
        .iter()
        .map(|floor| match floor.as_list()? {
            [id, w, h] => Some((
                id.as_window()?,
                Minimum {
                    w: w.as_num()?,
                    h: h.as_num()?,
                },
            )),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Plain;
    use crate::scroller::Scroller;
    use crate::tree::Tiling;
    use crate::{Minimum, Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }

    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            ..Settings::default()
        }
    }

    /// A tree of four windows, a seam moved and a floor set, so nothing in
    /// it is what a fresh tree would have.
    fn tree() -> Tiling {
        let mut tree = Tiling::new();
        tree.insert(1, None, None, area(), settings());
        for id in 2..=4 {
            tree.insert_largest(id, area(), settings());
        }
        tree.remove(2);
        tree.insert_largest(5, area(), settings());
        tree.set_floors([(3, Minimum { w: 420.0, h: 90.0 })].into_iter().collect());
        tree
    }

    #[test]
    fn a_tree_comes_back_from_plain_data_as_the_tree_it_was() {
        let tree = tree();
        let back = Tiling::from_plain(&tree.to_plain()).expect("a tree");
        assert_eq!(back.windows(), tree.windows());
        assert_eq!(
            back.layout(area(), settings()),
            tree.layout(area(), settings())
        );
        assert_eq!(back.to_plain(), tree.to_plain());
        assert_eq!(
            Tiling::from_plain(&Tiling::new().to_plain())
                .expect("an empty tree")
                .windows(),
            Vec::<u64>::new()
        );
    }

    /// What `change` makes of the tree's data is not a tree.
    fn refused(change: impl FnOnce(&mut Vec<Plain>)) -> bool {
        let Plain::List(mut parts) = tree().to_plain() else {
            panic!("a tree is a list");
        };
        change(&mut parts);
        Tiling::from_plain(&Plain::List(parts)).is_none()
    }

    fn nodes(parts: &mut [Plain]) -> &mut Vec<Plain> {
        match parts.get_mut(1) {
            Some(Plain::List(nodes)) => nodes,
            _ => panic!("the nodes are a list"),
        }
    }

    #[test]
    fn data_that_is_not_a_tree_is_refused() {
        assert!(!refused(|_| {}), "the tree itself is one");
        assert!(
            refused(|parts| parts[0] = Plain::Int(99)),
            "a root past the end"
        );
        assert!(
            refused(|parts| nodes(parts).push(Plain::List(vec![Plain::Int(77)]))),
            "a window nothing leads to"
        );
        assert!(
            refused(|parts| {
                let root = parts[0].clone();
                let nodes = nodes(parts);
                // A branch whose both sides are the root: a ring.
                nodes.push(Plain::List(vec![
                    Plain::Int(0),
                    Plain::Num(0.5),
                    root.clone(),
                    root,
                ]));
                let ring = Plain::place(nodes.len() - 1);
                parts[0] = ring;
            }),
            "a node led to twice"
        );
        assert!(
            refused(|parts| {
                for node in nodes(parts).iter_mut() {
                    if let Plain::List(split) = node
                        && split.len() == 4
                    {
                        split[1] = Plain::Num(f64::NAN);
                    }
                }
            }),
            "a ratio that is not a number"
        );
        assert!(Tiling::from_plain(&Plain::Int(3)).is_none());
    }

    fn strip() -> Scroller {
        let mut strip = Scroller::with_widths(&[0.25, 0.5, 0.75], 1);
        for id in 1..=4 {
            strip.insert(id, area(), settings());
        }
        strip.insert_into_active(5, area(), settings());
        strip.remove(2);
        strip
    }

    #[test]
    fn a_strip_comes_back_from_plain_data_as_the_strip_it_was() {
        let strip = strip();
        let back = Scroller::from_plain(&strip.to_plain()).expect("a strip");
        assert_eq!(
            back.layout(area(), settings()),
            strip.layout(area(), settings())
        );
        assert_eq!(back.to_plain(), strip.to_plain());
        assert!(Scroller::from_plain(&Scroller::new().to_plain()).is_some());
    }

    #[test]
    fn data_that_is_not_a_strip_is_refused() {
        let broken = |change: fn(&mut Vec<Plain>)| {
            let Plain::List(mut parts) = strip().to_plain() else {
                panic!("a strip is a list");
            };
            change(&mut parts);
            Scroller::from_plain(&Plain::List(parts)).is_none()
        };
        assert!(!broken(|_| {}), "the strip itself is one");
        assert!(
            broken(|parts| parts[1] = Plain::Int(40)),
            "a focus past the end"
        );
        assert!(
            broken(|parts| parts[3] = Plain::List(Vec::new())),
            "no width to cycle through"
        );
        assert!(
            broken(|parts| {
                if let Plain::List(columns) = &mut parts[0] {
                    columns.push(Plain::List(vec![
                        Plain::List(Vec::new()),
                        Plain::Int(0),
                        Plain::Num(0.5),
                    ]));
                }
            }),
            "a column with no window in it"
        );
    }
}
