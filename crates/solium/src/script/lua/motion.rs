//! How Lua writes a motion: rectangles, easings, transforms, deforms and
//! the options of a present or a warp.

use std::time::Duration;

use mlua::{Table, Value};

use crate::mat4::Mat4;
use crate::present::Curve;
use crate::script::lua::*;
use crate::script::*;

pub(crate) fn rect_from(options: &Table) -> mlua::Result<Option<Rect>> {
    let (x, y, w, h) = (
        options.get::<Option<f64>>("x")?,
        options.get::<Option<f64>>("y")?,
        options.get::<Option<f64>>("w")?,
        options.get::<Option<f64>>("h")?,
    );
    match (x, y, w, h) {
        // A size below zero, or any side that is not a number, is a mistake
        // in the script -- most often an easing's overshoot computed in Lua
        // -- and drawing it would panic deep in the renderer (#282). Said by
        // name, as a half-specified rect is.
        (Some(x), Some(y), Some(w), Some(h))
            if ![x, y, w, h].into_iter().all(f64::is_finite) || w < 0.0 || h < 0.0 =>
        {
            Err(mlua::Error::runtime(format!(
                "a rect needs finite x and y and a size of zero or more; got x={x}, y={y}, w={w}, h={h}"
            )))
        }
        (Some(x), Some(y), Some(w), Some(h)) => Ok(Some(Rect { x, y, w, h })),
        (None, None, None, None) => Ok(None),
        // A half-specified rect is a typo, and silently filling in the missing
        // half would put a window somewhere nobody asked for.
        _ => Err(mlua::Error::runtime(
            "a rect needs all of x, y, w and h, or none of them",
        )),
    }
}

/// Look up a curve by the name a script used.
///
/// The names come from the animation engine rather than a list kept here, so a
/// curve added there is immediately available to scripts — including springs,
/// which is why this is a lookup and not a match.
pub(crate) fn parse_easing(name: &str) -> Option<Curve> {
    Curve::from_name(name).or_else(|| {
        tracing::warn!(easing = name, "unknown easing, keeping the default");
        None
    })
}

/// Read an easing from an options table: a name, or a curve of your own.
///
/// ```lua
/// sol.animate({ duration = 240, easing = "outBack" })
/// sol.animate({ duration = 240, easing = { 0.34, 1.56, 0.64, 1 } })
/// ```
///
/// The four numbers are the control points of a cubic bezier -- what CSS
/// calls `cubic-bezier`, and what every easing generator on the internet
/// hands out. Named curves stay for the handful worth naming; this is so a
/// feel nobody anticipated does not need a compositor release.
pub(crate) fn easing_from(options: &Table) -> mlua::Result<Option<Curve>> {
    // Read as a value and matched, not asked for as a String and then as a
    // Table: asking a table for a String does not answer "no", it *fails*, and
    // the failure takes down the whole call it was made from. Which is how
    // every curve given as four numbers silently did nothing.
    let points = match options.get::<Value>("easing")? {
        Value::Nil => return Ok(None),
        Value::String(name) => return Ok(parse_easing(&name.to_string_lossy())),
        Value::Table(points) => points,
        other => {
            tracing::warn!(
                kind = other.type_name(),
                "an easing is a name or four numbers; keeping the default"
            );
            return Ok(None);
        }
    };
    if let Some(options) = points.get::<Option<Table>>("spring")? {
        let defaults = solium_animation::Spring::default();
        let spring = solium_animation::Spring {
            stiffness: options
                .get::<Option<f64>>("stiffness")?
                .unwrap_or(defaults.stiffness),
            damping: options
                .get::<Option<f64>>("damping")?
                .unwrap_or(defaults.damping),
            mass: options.get::<Option<f64>>("mass")?.unwrap_or(defaults.mass),
            initial_velocity: options
                .get::<Option<f64>>("initial_velocity")?
                .unwrap_or(defaults.initial_velocity),
            epsilon: options
                .get::<Option<f64>>("epsilon")?
                .unwrap_or(defaults.epsilon),
        };
        if ![
            spring.stiffness,
            spring.damping,
            spring.mass,
            spring.initial_velocity,
            spring.epsilon,
        ]
        .into_iter()
        .all(f64::is_finite)
            || spring.stiffness <= 0.0
            || spring.mass < f64::EPSILON
            || spring.damping < 0.0
            || spring.epsilon <= 0.0
            || spring.epsilon >= 1.0
        {
            return Err(mlua::Error::runtime(
                "spring: finite positive stiffness, mass >= machine epsilon, nonnegative damping, finite velocity and 0 < epsilon < 1 required",
            ));
        }
        return Ok(Some(Curve::Spring(spring)));
    }
    let numbers: Vec<f64> = points.sequence_values::<f64>().collect::<Result<_, _>>()?;
    let [x1, y1, x2, y2] = numbers.as_slice() else {
        tracing::warn!(
            found = numbers.len(),
            "an easing curve is four numbers; keeping the default"
        );
        return Ok(None);
    };
    // x outside 0..1 is not a curve a clock can walk along: time would have to
    // run backwards to reach it.
    Ok(Some(Curve::Bezier {
        x1: x1.clamp(0.0, 1.0),
        y1: *y1,
        x2: x2.clamp(0.0, 1.0),
        y2: *y2,
    }))
}

/// Read a 3D transform out of a script's options.
///
/// Degrees, because a configuration file written in radians is a
/// configuration file nobody edits twice. `perspective` is the viewer
/// distance in pixels — smaller is a wider lens and a harsher
/// foreshortening; without it the rotation is orthographic and a card turned
/// edge-on simply gets thinner instead of receding.
///
/// Returns `None` when nothing was asked for, so a window with no transform
/// stays on the renderer's cheap path.
pub(crate) fn transform_from(options: &Table) -> mlua::Result<Option<Mat4>> {
    let degrees = |name: &str| -> mlua::Result<Option<f32>> { options.get::<Option<f32>>(name) };
    let (rx, ry, rz) = (
        degrees("rotate_x")?,
        degrees("rotate_y")?,
        degrees("rotate_z")?,
    );
    let perspective = degrees("perspective")?;
    if rx.is_none() && ry.is_none() && rz.is_none() && perspective.is_none() {
        return Ok(None);
    }

    let radians = |value: Option<f32>| value.unwrap_or(0.0).to_radians();
    let mut matrix = Mat4::rotate_x(radians(rx))
        .then(Mat4::rotate_y(radians(ry)))
        .then(Mat4::rotate_z(radians(rz)));
    if let Some(distance) = perspective {
        matrix = matrix.then(Mat4::perspective(distance));
    }
    Ok(Some(matrix))
}

/// How deep a script asked for a window to be drawn. See
/// [`crate::present::Frame::z`].
///
/// Nothing said — no table at all, or a table that does not mention it — is
/// `0.0`, which ties with every other window and so leaves the order the stack
/// gave them exactly as it was. That is what makes the default free.
///
/// **A NaN is dropped and reported, and this guard is load-bearing.**
/// `render::by_depth` compares with `partial_cmp(..).unwrap_or(Equal)`, so a
/// NaN ties with `1.0` and with `2.0` while those two do not tie with each
/// other — a comparator that is not a total order. `slice::sort_by`'s contract
/// for one of those is not merely an unspecified arrangement: **it may panic**,
/// and the call site is the render walk, where a panic is the session going
/// black rather than a stack in the wrong order. One division in a script
/// reaches it. So the window keeps its place rather than the script keeping its
/// typo — the answer `deform_from` gives an effect this build does not have,
/// and not a cosmetic tidy-up to be relaxed later.
///
/// **An infinity is kept, either sign.** It orders against every finite depth
/// and never reaches any arithmetic — `z` is read in exactly one place, as the
/// sort key — so `z = math.huge` means "above everything", `-math.huge` means
/// "behind everything", and both cost nothing to honour.
pub(crate) fn depth_from(options: Option<&Table>) -> mlua::Result<f32> {
    /// Ties with every other window, so the stack's own order survives.
    const LEVEL: f32 = 0.0;

    let Some(options) = options else {
        return Ok(LEVEL);
    };
    let Some(z) = options.get::<Option<f32>>("z")? else {
        return Ok(LEVEL);
    };
    if z.is_nan() {
        tracing::warn!(
            "a NaN `z` cannot be ordered against anything; drawing at the default depth"
        );
        return Ok(LEVEL);
    }
    Ok(z)
}

/// What a script asked a window's matrix to turn about, as a fraction of the
/// rect it is drawn at. See [`crate::present::Frame::pivot`].
///
/// **Each axis defaults on its own.** `pivot_x = 0` means the left edge and
/// says nothing about the vertical; defaulting the pair together would take a
/// script that named one axis and hinge its window about a corner it never
/// mentioned.
///
/// **Outside `0..1` is kept, deliberately.** `pivot_x = 2` turns the window
/// about a line off to its right, which is a hinge and not a mistake — a door
/// swinging on a frame beside it — and `warp.rs` is exactly as defined there
/// as it is at the centre. Clamping would quietly turn one deliberate effect
/// into a different one.
///
/// **Non-finite is not kept.** `warp.rs` computes `loc + size * pivot` for the
/// point the matrix turns about, so a NaN or an infinity makes that point
/// non-finite and every vertex of the mesh with it. Nothing downstream
/// declines to draw the result: `Mat4::project_with_w` guards with
/// `out_w <= 1e-6`, and every comparison against a NaN is false. A window
/// would vanish, with a damage rectangle to match, because a script divided by
/// zero. That axis falls back to the centre and says so.
pub(crate) fn pivot_from(options: Option<&Table>) -> mlua::Result<(f32, f32)> {
    /// The middle of the window, which is what `warp.rs` computed before there
    /// was a pivot to name.
    const CENTRE: f32 = 0.5;

    let Some(options) = options else {
        return Ok((CENTRE, CENTRE));
    };
    // By key, so the two axes cannot be read into each other: there is one
    // body and it is given the name of the axis it is answering for.
    let axis = |key: &str| -> mlua::Result<f32> {
        let Some(fraction) = options.get::<Option<f32>>(key)? else {
            return Ok(CENTRE);
        };
        if !fraction.is_finite() {
            tracing::warn!(
                key,
                "a pivot that is not a finite fraction would put every vertex of the window at NaN; turning about the centre on that axis"
            );
            return Ok(CENTRE);
        }
        Ok(fraction)
    };
    Ok((axis("pivot_x")?, axis("pivot_y")?))
}

/// The keys of a `deform` that are not the effect's params.
pub(crate) const DEFORM_KEYS: [&str; 7] = [
    "effect",
    "to",
    "axis",
    "progress",
    "seed",
    "failed",
    "on_reload",
];

/// Read a deformation out of a `sol.present` options table.
///
/// ```lua
/// deform = { effect = "genie", axis = "down", spread = 1.4,
///            to = { x = 600, y = 1040, w = 120, h = 24 } }
/// ```
///
/// The effect is an effect folder, *named* rather than described: the name
/// is resolved by the effect host where the command is applied, your copy in
/// `~/.config/solium/effects/` before the shipped one. Every other key but
/// `to`, `axis`, `progress`, `seed`, `failed` and `on_reload` is one of that
/// effect's params, bound and checked by the effect, not here.
///
/// `to` is the **anchor**: what the window is being pulled into, or drawn out
/// of. See `present::Anchor` for why naming a thing rather than a rectangle is
/// the point of the key. `axis` is a word, `"auto"` among them; `progress`
/// is 1 when it is not given, all the way in, which is what one animates
/// towards; `seed`, `failed` and `on_reload` are refused outside their
/// words, naming the key (`tests::a_deforms_own_keys_are_refused_outside_their_words`).
pub(crate) fn deform_from(options: &Table) -> mlua::Result<Option<DeformSpec>> {
    use crate::effect::settings::{PresentFailed, PresentReload, said};
    let Some(deform) = options.get::<Option<Table>>("deform")? else {
        return Ok(None);
    };
    let Some(effect) = deform.get::<Option<String>>("effect")? else {
        return Err(mlua::Error::runtime(
            "a deform needs an `effect` name, such as { effect = \"genie\" }",
        ));
    };
    let Some(to) = deform.get::<Option<Table>>("to")? else {
        return Err(mlua::Error::runtime(
            "a deform needs a `to` to aim at: { window = id }, { surface = name } or a rect",
        ));
    };
    let refused =
        |key: &str, what: String| mlua::Error::runtime(format!("a deform's `{key}` is {what}"));
    let axis = deform.get::<Option<String>>("axis")?;
    if let Some(word) = &axis
        && word != "auto"
        && solium_effects::Axis::from_name(word).is_none()
    {
        let mut words = vec!["auto"];
        words.extend(solium_effects::Axis::all().map(|(name, _)| name));
        return Err(refused("axis", said(&words)));
    }
    let progress = deform.get::<Option<f64>>("progress")?.unwrap_or(1.0);
    if !progress.is_finite() {
        return Err(refused("progress", "a finite number".to_owned()));
    }
    let seed = deform.get::<Option<f64>>("seed")?;
    if seed.is_some_and(|seed| !(0.0..1.0).contains(&seed)) {
        return Err(refused("seed", "a number from 0 up to 1".to_owned()));
    }
    let word = |key: &str, words: &[&str]| -> mlua::Result<Option<String>> {
        match deform.get::<Option<String>>(key)? {
            Some(word) if !words.contains(&word.as_str()) => Err(refused(key, said(words))),
            other => Ok(other),
        }
    };
    let failed = word("failed", &PresentFailed::WORDS)?
        .as_deref()
        .and_then(PresentFailed::from_word);
    let on_reload = word("on_reload", &PresentReload::WORDS)?
        .as_deref()
        .and_then(PresentReload::from_word);
    let mut params = Vec::new();
    for pair in deform.pairs::<String, Value>() {
        let (key, value) = pair?;
        if DEFORM_KEYS.contains(&key.as_str()) {
            continue;
        }
        match crate::script::doc_of_plain(&value)?
            .as_ref()
            .and_then(crate::effect::param_value)
        {
            Some(value) => params.push((key, value)),
            None => {
                return Err(refused(
                    &key,
                    "not a param's value: a number, a boolean, a word or four numbers".to_owned(),
                ));
            }
        }
    }
    params.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(DeformSpec {
        effect,
        params,
        axis,
        progress,
        aim: aim_from(&to)?,
        seed,
        failed,
        on_reload,
    }))
}

/// Read a deform's anchor: a thing to follow, or a place to aim at.
///
/// The two identities are the ones that matter -- they are resolved on every
/// frame that draws, so the effect tracks a dock icon or another window as it
/// moves. A rect aims at somewhere that does not move, such as the bottom edge
/// of a monitor, and is honest about being a snapshot because there is nothing
/// there to track.
pub(crate) fn aim_from(to: &Table) -> mlua::Result<Aim> {
    if let Some(id) = to.get::<Option<u64>>("window")? {
        return Ok(Aim::Window(id));
    }
    if let Some(name) = to.get::<Option<String>>("surface")? {
        return Ok(Aim::Surface(name));
    }
    let Some(rect) = rect_from(to)? else {
        return Err(mlua::Error::runtime(
            "a deform's `to` needs { window = id }, { surface = name } or a rect (x, y, w, h)",
        ));
    };
    Ok(Aim::Rect(rect))
}

/// Read a selection out of a `sol.group` table.
///
/// ```lua
/// sol.group("desk-2", {
///     windows  = { 3, 7 },
///     surfaces = { "wallpaper-2" },
///     monitors = { "DP-1" },      -- everything drawn there
///     monitor  = "DP-1",          -- which instance of each surface above
/// })
/// ```
///
/// Every key is optional and an absent one is an empty list, so a selection of
/// nothing is spellable and does nothing — which is what a mode building one
/// desk per monitor per workspace produces for the cells that are empty.
pub(crate) fn selection_from(options: &Table) -> mlua::Result<Selection> {
    let names = |key: &str| -> mlua::Result<Vec<String>> {
        match options.get::<Option<Table>>(key)? {
            Some(list) => list.sequence_values::<String>().collect(),
            None => Ok(Vec::new()),
        }
    };
    let windows = match options.get::<Option<Table>>("windows")? {
        Some(list) => list.sequence_values::<u64>().collect::<mlua::Result<_>>()?,
        None => Vec::new(),
    };
    Ok(Selection {
        windows,
        surfaces: names("surfaces")?,
        monitors: names("monitors")?,
        on: options.get::<Option<String>>("monitor")?,
    })
}

/// Read what a selection is carried by out of a `sol.present_group` table.
///
/// ```lua
/// sol.present_group("desk-2", { x = -2560, opacity = 0.4, rotate_y = 8 })
/// ```
///
/// `x` and `y` are a **displacement** and not a destination, which is the one
/// way this reads differently from `sol.present`: a selection has no rectangle
/// of its own to be moved to. `rotate_*` and `perspective` are read by the same
/// `transform_from` a window's own matrix comes from, so the two spell a
/// rotation identically.
///
/// **`z`, `pivot_x` and `pivot_y` are deliberately not read here**, and it is
/// not an oversight to be tidied up by copying the two lines from
/// `sol.present`. They are the two fields of a frame a selection cannot carry:
///
/// * A **pivot** would have to replace each member's own, because a member is
///   drawn through one matrix turning about one point, and `Shift::apply`
///   composes the group's matrix onto the member's. That contradicts the
///   promise this whole primitive is built on — "a window tilted inside a
///   moving desk stays tilted *within* it" — and it still would not be the
///   thing a script asking for it wants, which is the desk turning as one
///   about a point. That needs a *rectangle for the group*, which no selection
///   has; `group.rs`'s `Shift::matrix` already names it as the honest limit of
///   this stage.
/// * A **depth** would reach only some of a selection. `z` orders the pane
///   walk in `render.rs` and nothing else: scripted surfaces are drawn in
///   fixed layer passes, in declaration order, carried and faded but never
///   sorted. So a desk raised by `z` would lift its windows above the desk
///   next door and leave its own wallpaper behind — the one failure a
///   selection exists to make impossible.
///
/// Both are a *node's* answer, which is where the spec puts them, and
/// `sol.present` is how a script gives one. An unknown key in this table is
/// ignored like any other, so a script that writes one gets no window in the
/// wrong place — it gets nothing, which is the mild half of this note.
pub(crate) fn shift_from(options: &Table) -> mlua::Result<crate::group::Shift> {
    Ok(crate::group::Shift {
        dx: options.get::<Option<f64>>("x")?.unwrap_or(0.0),
        dy: options.get::<Option<f64>>("y")?.unwrap_or(0.0),
        opacity: options.get::<Option<f32>>("opacity")?.unwrap_or(1.0),
        matrix: transform_from(options)?.unwrap_or(Mat4::IDENTITY),
    })
}

/// What a call said about its own timing, before it is laid over the ambient
/// one `sol.animate` set.
///
/// Read outside the pending buffer and applied inside it, because reading a Lua
/// table can fail and the buffer is held by a closure that cannot. Two options
/// rather than an `AnimationSpec`, so "said nothing about the easing" and
/// "asked for the default easing" stay different answers.
///
/// `sol.present_group` takes a table of its own because a mode carrying two
/// selections at different speeds in one dispatch cannot say so with an ambient
/// setting. Absent, it is the ambient setting, so the two calls feel the same as
/// every other pair in this file.
pub(crate) fn motion_from(
    options: Option<&Table>,
) -> mlua::Result<(Option<Duration>, Option<Curve>)> {
    let Some(options) = options else {
        return Ok((None, None));
    };
    Ok((
        options
            .get::<Option<u64>>("duration")?
            .map(Duration::from_millis),
        easing_from(options)?,
    ))
}

/// A present's `motion` with what it leaves out filled from the batch's own
/// timing (#293).
pub(crate) fn resolved(
    motion: Option<PresentMotion>,
    animation: AnimationSpec,
) -> Option<Box<crate::present::Channels<AnimationSpec>>> {
    motion.map(|motion| {
        Box::new(motion.map(|(duration, easing)| AnimationSpec {
            duration: duration.unwrap_or(animation.duration),
            easing: easing.unwrap_or(animation.easing),
        }))
    })
}

/// A present's or a `present_from`'s `motion` (#293): a timing per property, `x` and `y` for the
/// drawn rect's centre, `size` for its width and height, `opacity`, and
/// `client_opacity` for its client's inside it (#296), each
/// `{ duration, easing }` as `sol.animate` takes it. A property left out, and
/// a duration or easing left out of one, is `None` for the caller to fill
/// with the batch's own; no `motion` at all is `None`. Anything else under
/// `motion` is an error naming it.
/// `tests::a_present_motion_names_its_properties_and_a_bad_one_is_refused`.
pub(crate) type PresentMotion = crate::present::Channels<(Option<Duration>, Option<Curve>)>;

pub(crate) fn present_motion_from(options: &Table) -> mlua::Result<Option<PresentMotion>> {
    let motion = match options.get::<Value>("motion")? {
        Value::Nil => return Ok(None),
        Value::Table(motion) => motion,
        other => {
            return Err(mlua::Error::runtime(format!(
                "sol.present: `motion` is a table of timings by property ({}), got a {}",
                PRESENT_MOTIONS.join(", "),
                other.type_name()
            )));
        }
    };
    for pair in motion.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(name) = &key else {
            return Err(mlua::Error::runtime(format!(
                "sol.present: `motion` is keyed by property ({}), got a key of type {}",
                PRESENT_MOTIONS.join(", "),
                key.type_name()
            )));
        };
        let name = name.to_string_lossy();
        if !PRESENT_MOTIONS.contains(&name.as_str()) {
            return Err(mlua::Error::runtime(format!(
                "sol.present: `motion` times {}, not {name}",
                PRESENT_MOTIONS.join(", ")
            )));
        }
        if !matches!(value, Value::Table(_)) {
            return Err(mlua::Error::runtime(format!(
                "sol.present: `motion.{name}` is {{ duration, easing }}, got a {}",
                value.type_name()
            )));
        }
    }
    let timing = |name: &str| motion_from(motion.get::<Option<Table>>(name)?.as_ref());
    Ok(Some(crate::present::Channels {
        x: timing("x")?,
        y: timing("y")?,
        size: timing("size")?,
        opacity: timing("opacity")?,
        client_opacity: timing("client_opacity")?,
    }))
}

/// A present's `client_opacity` (#296): the client's opacity inside its pane,
/// a number from 0 to 1, or `None` when the table names none. Anything else
/// is refused by name, as a half-given rect is: a window whose content
/// stayed opaque, or vanished, because a script passed a string or a 2 is a
/// mistake nobody would find from the screen.
/// `tests::a_client_opacity_is_a_number_from_0_to_1_and_anything_else_is_refused`.
pub(crate) fn client_opacity_from(options: &Table, call: &str) -> mlua::Result<Option<f32>> {
    let value = options.get::<Value>("client_opacity")?;
    if value.is_nil() {
        return Ok(None);
    }
    let Some(number) = number(&value) else {
        return Err(mlua::Error::runtime(format!(
            "{call}: `client_opacity` is a number from 0 to 1, got a {}",
            value.type_name()
        )));
    };
    if !(0.0..=1.0).contains(&number) {
        return Err(mlua::Error::runtime(format!(
            "{call}: `client_opacity` is a number from 0 to 1, got {number}"
        )));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a number from 0 to 1, checked above"
    )]
    Ok(Some(number as f32))
}

/// Where on a window the pointer goes when no `to` says: its centre.
pub(crate) const CENTRE: (f64, f64) = (0.5, 0.5);

/// A warp's options as [`warp_options`] reads them: where on a window, as a
/// share of its width and height, the motion its picture flies along, and
/// where real motion mid-flight carries on from.
pub(crate) type WarpOptions = (Option<(f64, f64)>, Option<AnimationSpec>, Interrupt);

/// A warp's options (#220): `to`, where on a window, `"center"` or a share
/// of its width and height `{ x, y }`; `motion`, how the drawn pointer
/// flies there, `{ duration, easing }`; and `interrupt`, where the mouse
/// carries on from if it moves before the picture lands, `"pointer"` or
/// `"picture"`. `to` left out is `None` for the caller to fill in; `motion`
/// left out is none, and the picture is there at once; `interrupt` left out
/// is `"pointer"`. A duration left out of a motion is the default 220 ms,
/// and an easing the default `"outCubic"`, as `sol.animate`'s are.
/// `tests::sol_warp_names_a_window_or_a_point_and_a_bad_one_is_an_error`.
pub(crate) fn warp_options(options: Option<&Table>, caller: &str) -> mlua::Result<WarpOptions> {
    let Some(options) = options else {
        return Ok((None, None, Interrupt::default()));
    };
    let at = match options.get::<Value>("to")? {
        Value::Nil => None,
        Value::String(name) if name.to_string_lossy() == "center" => Some(CENTRE),
        Value::Table(share) => match (
            share.get::<Option<f64>>("x")?,
            share.get::<Option<f64>>("y")?,
        ) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
            _ => {
                return Err(mlua::Error::runtime(format!(
                    "{caller}: `to` as a share of the window is {{ x = <number>, y = <number> }}"
                )));
            }
        },
        other => {
            return Err(mlua::Error::runtime(format!(
                "{caller}: `to` is \"center\" or {{ x, y }}, not {}",
                describe(&other)
            )));
        }
    };
    let motion = match options.get::<Value>("motion")? {
        Value::Nil => None,
        Value::Table(motion) => {
            let (duration, easing) = motion_from(Some(&motion))?;
            Some(AnimationSpec::default().with(duration, easing))
        }
        other => {
            return Err(mlua::Error::runtime(format!(
                "{caller}: `motion` is {{ duration, easing }}, not {}",
                other.type_name()
            )));
        }
    };
    let interrupt = match options.get::<Value>("interrupt")? {
        Value::Nil => Interrupt::default(),
        Value::String(name) if name.to_string_lossy() == "pointer" => Interrupt::Pointer,
        Value::String(name) if name.to_string_lossy() == "picture" => Interrupt::Picture,
        other => {
            return Err(mlua::Error::runtime(format!(
                "{caller}: `interrupt` is \"pointer\" or \"picture\", not {}",
                describe(&other)
            )));
        }
    };
    Ok((at, motion, interrupt))
}

/// The deform `sol.present(id, { deform = <lua> })` carries, for a state
/// test to apply as the command would.
#[cfg(test)]
pub(crate) fn deform_for_test(lua: &str) -> DeformSpec {
    let state = mlua::Lua::new();
    let options: Table = state
        .load(format!("return {{ deform = {lua} }}"))
        .eval()
        .expect("the test's deform");
    deform_from(&options)
        .expect("a deform")
        .expect("a deform given")
}
