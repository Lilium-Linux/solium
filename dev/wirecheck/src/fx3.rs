//! FX3's GPU cases: the memo a T2 slot reads and the crop of it a part draws,
//! a backdrop drawn beneath a warp with the hole its capture keeps (13i), and
//! the shipped shade's edge (13j), which no unit test can reach. Run alone with `WIRECHECK_ONLY=fx3`, and after
//! FX2's cases in a full run. Each prints a `=== FX3:` heading. The effect
//! programs, the executor and the effect element are the compositor's own, as
//! FX2's cases include them (`crate::fx2::{gl, run, element}`).

use anyhow::{Result, anyhow};
use smithay::backend::renderer::Texture as _;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::utils::Rectangle;
use solium_effects::stage::{self, Split};

use crate::fx0::pool;
use crate::fx2::{self, element, run};

/// The monitor 13a's xray covers, in pixels.
const MONITOR: (usize, usize) = (640, 360);

/// The part 13a and 13b crop: x, y, w, h. Off the monitor's centre in both
/// directions, so a crop whose y were measured from the bottom edge (rows 240
/// to 320) reads other pixels, which 13b checks it would.
const PART: (usize, usize, usize, usize) = (200, 40, 120, 80);

/// The shipped blur's `passes`, `offset` and reach (`offset · 2^(passes + 1)`,
/// its `effect.lua`'s), at its defaults.
const PASSES: u32 = 3;
const OFFSET: f64 = 3.0;
const REACH: usize = 48;

/// Every FX3 case, in order.
pub(crate) fn all(renderer: &mut GlesRenderer) -> Result<()> {
    let memo = the_memo_matches_a_whole_monitor_blur(renderer)?;
    a_crop_of_the_memo_draws_its_own_pixels(renderer, &memo)?;
    a_backdrop_beneath_a_mesh_and_its_hole(renderer)?;
    the_shipped_shade_is_a_dark_edge_round_the_outline(renderer)?;
    Ok(())
}

/// **Case 13j: the shipped shade is a dark edge just inside the part's
/// rounded outline.** `shade/` at its defaults (the `glass` style's width 12,
/// strength 0.5, black), run once as `render::run_slots` runs a T0 chain:
/// its first input the pool's blank, over a 400×300 part with 16-pixel
/// corners and no reach. Read back, premultiplied: on all four sides the
/// very edge is dark (alpha 100 or more, no channel above 8); 6 px in it is
/// fainter by more than 24 and not gone; 14 px in, and in the middle,
/// nothing; and at the top-left corner nothing outside its curve at (1, 1)
/// and dark on it at (5, 5). What the style's `Shade.qml` drew in QML until
/// 2026-10-15, now one pass on the GPU, run once per size. With `width = 0`,
/// nothing anywhere, and with `strength = 1` darker at the edge.
fn the_shipped_shade_is_a_dark_edge_round_the_outline(renderer: &mut GlesRenderer) -> Result<()> {
    use solium_effects::spec::Value;
    println!("\n=== FX3: the shipped shade, a dark edge just inside the outline ===");
    let defaults = || {
        vec![
            ("color".to_owned(), Value::Vec4([0.0, 0.0, 0.0, 1.0])),
            ("falloff".to_owned(), Value::Number(2.0)),
            ("radius".to_owned(), Value::Number(-1.0)),
            ("strength".to_owned(), Value::Number(0.5)),
            ("width".to_owned(), Value::Number(12.0)),
        ]
    };
    let with = |name: &str, value: f64| {
        let mut params = defaults();
        for (each, given) in &mut params {
            if each == name {
                *given = Value::Number(value);
            }
        }
        params
    };
    let size = (400_usize, 300_usize);
    // RGBA at (x, y) of the shade drawn with `params`.
    let draw = |renderer: &mut GlesRenderer, params| -> Result<Vec<u8>> {
        let (plan, programs) = fx2::build(renderer, fx2::shipped_shade(params))?;
        let mut pool = pool::Pool::new(0);
        let blank = pool.blank(renderer).ok_or_else(|| anyhow!("no blank texture"))?;
        let textures = [(plan.first_input.as_str(), blank, run::BoxMap::WHOLE)];
        let inputs = run::Inputs {
            radii: [16.0; 4],
            ..fx2::whole(size, &textures)
        };
        let mut held = run::Held::default();
        let outcome = fx2::run_once(
            renderer,
            &mut pool,
            &|key| fx2::lookup(&programs, key),
            &plan,
            &mut held,
            &inputs,
        )?;
        let run::Outcome::Done(texture, _, _) = outcome else {
            return Err(anyhow!("the shade did not run: {outcome:?}"));
        };
        let bytes = fx2::read_rgba(renderer, &texture)?;
        drop(held);
        pool.sweep(renderer);
        fx2::free(renderer, programs)?;
        Ok(bytes)
    };
    let pixel = |bytes: &[u8], x: usize, y: usize| -> [u8; 4] {
        let at = (y * size.0 + x) * 4;
        bytes
            .get(at..at + 4)
            .and_then(|pixel| <[u8; 4]>::try_from(pixel).ok())
            .unwrap_or_default()
    };
    let shade = draw(renderer, defaults())?;
    for (x, y) in [(200, 0), (0, 150), (399, 150), (200, 299)] {
        let [r, g, b, a] = pixel(&shade, x, y);
        if a < 100 || r.max(g).max(b) > 8 {
            return Err(anyhow!("({x}, {y}) is {:?}, not a dark edge", [r, g, b, a]));
        }
    }
    let (edge, inward) = (pixel(&shade, 200, 0)[3], pixel(&shade, 200, 6)[3]);
    if inward == 0 || inward + 24 >= edge {
        return Err(anyhow!("it does not fall away inward: {edge} at the edge, {inward} 6 px in"));
    }
    for (x, y) in [(200, 14), (14, 150), (200, 150)] {
        let a = pixel(&shade, x, y)[3];
        if a != 0 {
            return Err(anyhow!("it reaches ({x}, {y}): alpha {a}"));
        }
    }
    let (outside, on) = (pixel(&shade, 1, 1)[3], pixel(&shade, 5, 5)[3]);
    if outside > 8 || on < 60 {
        return Err(anyhow!("it does not follow the round corner: {outside} outside it, {on} on it"));
    }
    let none = draw(renderer, with("width", 0.0))?;
    if let Some(at) = none.chunks_exact(4).position(|pixel| pixel[3] != 0) {
        return Err(anyhow!("width 0 drew at ({}, {})", at % size.0, at / size.0));
    }
    let strong = pixel(&draw(renderer, with("strength", 1.0))?, 200, 0)[3];
    if strong <= edge {
        return Err(anyhow!("strength 1 is no darker at the edge: {strong} against {edge}"));
    }
    println!(
        "  edge alpha {edge}, 6 px in {inward}, none 14 px in or in the middle, round at the \
         corner ({outside} outside, {on} on it); width 0 draws nothing; strength 1 gives {strong}"
    );
    Ok(())
}

/// 13a's xray at `(x, y)`, anywhere, the monitor's corner at (0, 0): a
/// checker of 64-pixel cells, 0 and 255 at a different phase in each
/// channel. Cells this size keep most of their contrast through the shipped
/// blur, whose kernel is some 17 pixels wide: 8 or 16-pixel cells come out
/// grey within one everywhere, so a result read from the wrong place, or not
/// blurred at all where it should be, would agree with the reference anyway.
fn checker(x: i64, y: i64) -> [u8; 4] {
    const PHASES: [(i64, i64); 4] = [(0, 0), (21, 43), (43, 11), (11, 53)];
    std::array::from_fn(|k| {
        let (px, py) = PHASES[k];
        if ((x + px).div_euclid(64) + (y + py).div_euclid(64)).rem_euclid(2) == 0 {
            255
        } else {
            0
        }
    })
}

/// `bytes` (RGBA, rows of `width`) inside `(x, y, w, h)`, row by row.
fn sub(bytes: &[u8], width: usize, (x, y, w, h): (usize, usize, usize, usize)) -> Vec<u8> {
    (y..y + h)
        .flat_map(|row| {
            let at = (row * width + x) * 4;
            bytes.get(at..at + w * 4).unwrap_or_default().to_vec()
        })
        .collect()
}

/// The largest channel difference between two pictures of the same size.
fn worst(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(p, q)| p.abs_diff(*q))
        .max()
        .unwrap_or(0)
}

/// What 13a leaves 13b: the memo as drawn on the GPU, read back.
struct Memo {
    texture: GlesTexture,
    bytes: Vec<u8>,
}

/// **Case 13a: the memo matches a whole-monitor CPU blur.** The shipped
/// `blur/` at `passes = 3, offset = 3`, flattened as the host flattens it and
/// split as the compositor splits it (`stage::split_memo`, `Split::Resize`):
/// its prefix is the whole plan, its suffix empty, so every part reads a crop
/// of the memo and runs nothing. The prefix runs as `render::run_xray` runs a
/// memo: at 640×360, `backdrop` the xray (a checker of 64-pixel cells, a
/// different phase in each channel) through `BoxMap::WHOLE`. Read back, every
/// channel is within 1 of `fx2::cpu_kawase` over the **whole** picture, and
/// so is the part's 120×80 box at (200, 40) of each (a crop is the memo's own
/// pixels). Printed, not asserted: the largest difference between that crop
/// and a per-part run of the same blur over the box padded by its reach,
/// which clamps where the memo reads real pixels ([R] §2): why 13a holds the
/// memo to the whole monitor and never to a per-part run.
fn the_memo_matches_a_whole_monitor_blur(renderer: &mut GlesRenderer) -> Result<Memo> {
    println!("\n=== FX3: the memo matches a whole-monitor CPU blur within one ===");
    let side = |n: usize| i64::try_from(n).unwrap_or(0);
    let (xray, xray_bytes) = fx2::upload(renderer, MONITOR, |x, y| checker(side(x), side(y)))?;
    let (plan, programs) = fx2::build(renderer, fx2::shipped_blur(PASSES, OFFSET))?;
    let split = stage::split_memo(&plan, Split::Resize);
    let (Some(prefix), [export]) = (split.prefix.as_ref(), split.exports.as_slice()) else {
        return Err(anyhow!(
            "the shipped blur did not split into a prefix and one export: {split:?}"
        ));
    };
    if !split.suffix.steps.is_empty() || export.step + 1 != plan.steps.len() {
        return Err(anyhow!(
            "the shipped blur's memo is not its whole plan: export {export:?}, suffix {:?}",
            split.suffix.steps
        ));
    }
    // What `want_t2` hands the memo, and `run_xray` runs.
    let memo_plan = prefix.upto(export.step);
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("backdrop", xray, run::BoxMap::WHOLE)];
    let outcome = fx2::run_once(
        renderer,
        &mut pool,
        &|key| fx2::lookup(&programs, key),
        &memo_plan,
        &mut held,
        &fx2::whole(MONITOR, &textures),
    )?;
    let run::Outcome::Done(texture, _, _) = outcome else {
        return Err(anyhow!("the memo's run came to {outcome:?}"));
    };
    if texture.size() != (640, 360).into() {
        return Err(anyhow!("the memo is {:?}, not 640×360", texture.size()));
    }
    let bytes = fx2::read_rgba(renderer, &texture)?;
    let cpu = fx2::cpu_kawase(&plan, &xray_bytes, MONITOR, OFFSET);
    // Rounded as an rgba8 target stores it, each pass's result already is.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0 to 255"
    )]
    let reference: Vec<u8> = cpu
        .pixels
        .iter()
        .flat_map(|pixel| pixel.map(|channel| channel as u8))
        .collect();
    let mut off = (0_u8, (0, 0));
    for (at, (got, want)) in bytes
        .chunks_exact(4)
        .zip(reference.chunks_exact(4))
        .enumerate()
    {
        let here = worst(got, want);
        if here > off.0 {
            off = (here, (at % MONITOR.0, at / MONITOR.0));
        }
    }
    if off.0 > 1 {
        return Err(anyhow!(
            "the memo differs from the whole-monitor CPU blur by {} at {:?}",
            off.0,
            off.1
        ));
    }
    let cropped = worst(
        &sub(&bytes, MONITOR.0, PART),
        &sub(&reference, MONITOR.0, PART),
    );
    if cropped > 1 {
        return Err(anyhow!(
            "the part's crop differs from the reference's by {cropped}"
        ));
    }
    let per_part = per_part_run(renderer, &programs, &plan)?;
    let edge = worst(&sub(&bytes, MONITOR.0, PART), &per_part);
    held.release(&mut pool);
    pool.sweep(renderer);
    fx2::free(renderer, programs)?;
    println!(
        "  three down and three up over 640×360: at most {} from the CPU's, at {:?}; the part's \
         120×80 crop at most {cropped}; a per-part run over the part padded by its reach differs \
         from the crop by up to {edge} (printed, not judged: it clamps where the memo reads on)",
        off.0, off.1
    );
    Ok(Memo { texture, bytes })
}

/// The blur run per part, as a part reads it when its memo is not held: its
/// whole plan over the part's box padded by the blur's reach, the xray's own
/// pixels there, clamped at that box's edge; the part's 120×80 of the result.
fn per_part_run(
    renderer: &mut GlesRenderer,
    programs: &std::collections::HashMap<u64, fx2::gl::Program>,
    plan: &solium_effects::stage::Plan,
) -> Result<Vec<u8>> {
    let side = |n: usize| i64::try_from(n).unwrap_or(0);
    let padded = (PART.2 + 2 * REACH, PART.3 + 2 * REACH);
    let corner = (side(PART.0) - side(REACH), side(PART.1) - side(REACH));
    let (input, _) = fx2::upload(renderer, padded, |x, y| {
        checker(corner.0 + side(x), corner.1 + side(y))
    })?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("backdrop", input, run::BoxMap::WHOLE)];
    let outcome = fx2::run_once(
        renderer,
        &mut pool,
        &|key| fx2::lookup(programs, key),
        plan,
        &mut held,
        &fx2::whole(padded, &textures),
    )?;
    let run::Outcome::Done(result, _, _) = outcome else {
        return Err(anyhow!("the per-part run came to {outcome:?}"));
    };
    let bytes = fx2::read_rgba(renderer, &result)?;
    drop(result);
    held.release(&mut pool);
    pool.sweep(renderer);
    Ok(sub(&bytes, padded.0, (REACH, REACH, PART.2, PART.3)))
}

/// **Case 13b: a crop of the memo draws its own pixels.** The compositor's
/// own `EffectElement` over 13a's memo, cropped to the part's box and then
/// placed under a mask, as the walk places a slot whose suffix is empty with
/// its part's mask (`EffectElement::new(..).cropped((200, 40, 120, 80))
/// .at((0, 0, 120, 80), Some(the whole box, square), 1.0)`): a mask is what
/// sends the draw through `MASKED_TEXTURE`, compiled as `Programs::masked`
/// compiles it, with the crop mapping the mask (smithay draws one with no
/// mask through its own program). Drawn by `pool::paint` into a 120×80
/// pooled target and read back: every channel within 1 of the memo's own
/// pixels in that box, so the mask, mapped through the crop, covers all of
/// it. The box is off the centre, and the memo's rows 240 to 320 (the box
/// measured from the bottom edge) differ from its rows 40 to 120 by far more
/// than 1, checked here, so a crop read upside down cannot pass. Case 12h
/// cuts under a crop.
fn a_crop_of_the_memo_draws_its_own_pixels(renderer: &mut GlesRenderer, memo: &Memo) -> Result<()> {
    use smithay::backend::renderer::Bind as _;
    use smithay::backend::renderer::Frame as _;
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::Physical;
    use solium_effects::fragment::{Corners, MASKED_TEXTURE};
    println!("\n=== FX3: a crop of the memo draws its own pixels ===");
    let want = sub(&memo.bytes, MONITOR.0, PART);
    let mirrored = (PART.0, MONITOR.1 - PART.1 - PART.3, PART.2, PART.3);
    let apart = worst(&want, &sub(&memo.bytes, MONITOR.0, mirrored));
    if apart <= 8 {
        return Err(anyhow!(
            "the box and its mirror in y differ by only {apart}, so a crop upside down would pass"
        ));
    }
    let program = renderer
        .compile_custom_texture_shader(
            MASKED_TEXTURE,
            &crate::uniforms::registration(MASKED_TEXTURE),
        )
        .map_err(|err| anyhow!("MASKED_TEXTURE did not compile in every variant: {err}"))?;
    let to_f64 = |n: usize| f64::from(u32::try_from(n).unwrap_or(0));
    let to_i32 = |n: usize| i32::try_from(n).unwrap_or(0);
    let crop = Rectangle::new(
        (to_f64(PART.0), to_f64(PART.1)).into(),
        (to_f64(PART.2), to_f64(PART.3)).into(),
    );
    let side = (to_i32(PART.2), to_i32(PART.3));
    // The whole box, square: every pixel's centre half a pixel or more
    // inside it, so fully covered.
    let mask =
        Rectangle::<f64, Physical>::new((0.0, 0.0).into(), (to_f64(PART.2), to_f64(PART.3)).into());
    // Cropped, then placed under the mask, as the walk places a memo's crop.
    let placed = element::EffectElement::new(
        Id::new(),
        CommitCounter::default(),
        memo.texture.clone(),
        program,
    )
    .cropped(crop)
    .at(
        Rectangle::from_size(side.into()),
        Some((mask, Corners::all(0.0))),
        1.0,
    );
    let mut pool = pool::Pool::new(0);
    let mut carrier = pool
        .carrier(renderer)
        .ok_or_else(|| anyhow!("no carrier"))?;
    let target = pool
        .target(&mut pool::Gl(renderer), side.into(), pool::Format::Rgba8)
        .ok_or_else(|| anyhow!("no pooled target"))?;
    {
        let mut bound = renderer
            .bind(&mut carrier)
            .map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        pool::paint(&mut frame, side.into(), &[placed], 1.0).map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let drawn = fx2::read_rgba(renderer, target.texture())?;
    drop(target);
    pool.sweep(renderer);
    let off = worst(&drawn, &want);
    if drawn.len() != want.len() || off > 1 {
        let at = drawn
            .chunks_exact(4)
            .zip(want.chunks_exact(4))
            .position(|(got, wanted)| worst(got, wanted) > 1)
            .map(|at| (at % PART.2, at / PART.2));
        return Err(anyhow!(
            "the memo's crop drawn differs from the memo's own pixels in that box by {off}, first at {at:?}"
        ));
    }
    println!(
        "  the 120×80 crop at (200, 40), drawn by smithay: at most {off} from the memo there; \
         its mirror in y differs by {apart}; through MASKED_TEXTURE under a mask of the box"
    );
    Ok(())
}

/// 13i's bent 2×2 grid: a 200×120 pane's nine points on a 640×360 target,
/// row by row (not affine, so a sample at the wrong place shows).
const GRID: [(f32, f32); 9] = [
    (100.0, 40.0),
    (220.0, 50.0),
    (340.0, 60.0),
    (130.0, 170.0),
    (215.0, 170.0),
    (320.0, 160.0),
    (160.0, 300.0),
    (230.0, 290.0),
    (300.0, 280.0),
];

/// 13i's mask: the part (20, 10, 160, 100) of the 200×120 pane, its
/// top-left and bottom-right corners rounded by 16.
const MASK: ([f32; 4], [f32; 4], [f32; 2]) = (
    [20.0, 10.0, 160.0, 100.0],
    [16.0, 0.0, 0.0, 16.0],
    [200.0, 120.0],
);

/// 13i's grid as triangles, each vertex (x, y, u, v), two a cell as
/// `warp::mesh_grid` cuts them.
fn grid_triangles() -> Vec<[f32; 4]> {
    let at = |column: usize, row: usize| {
        let (x, y) = GRID[row * 3 + column];
        #[expect(clippy::cast_precision_loss, reason = "0, 1 or 2")]
        let (u, v) = (column as f32 / 2.0, row as f32 / 2.0);
        [x, y, u, v]
    };
    let mut triangles = Vec::new();
    for row in 0..2 {
        for column in 0..2 {
            let (tl, tr, br, bl) = (
                at(column, row),
                at(column + 1, row),
                at(column + 1, row + 1),
                at(column, row + 1),
            );
            triangles.extend([tl, tr, br, tl, br, bl]);
        }
    }
    triangles
}

/// The texture coordinate the grid puts at `(x, y)`: the triangle holding
/// it, by its own barycentric weights; `None` outside the grid.
fn uv_at(triangles: &[[f32; 4]], (x, y): (f32, f32)) -> Option<(f32, f32)> {
    triangles.chunks_exact(3).find_map(|t| {
        let ([x0, y0, u0, v0], [x1, y1, u1, v1], [x2, y2, u2, v2]) = (t[0], t[1], t[2]);
        let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
        let b1 = ((x - x0) * (y2 - y0) - (x2 - x0) * (y - y0)) / area;
        let b2 = ((x1 - x0) * (y - y0) - (x - x0) * (y1 - y0)) / area;
        let b0 = 1.0 - b1 - b2;
        (b0 >= 0.0 && b1 >= 0.0 && b2 >= 0.0)
            .then(|| (b0 * u0 + b1 * u1 + b2 * u2, b0 * v0 + b1 * v1 + b2 * v2))
    })
}

/// How far `(px, py)`, in the pane's pixels, is inside 13i's mask: positive
/// inside, by the rounded rectangle's own field.
fn inside_mask((px, py): (f32, f32)) -> f32 {
    let ([x, y, w, h], radii, _) = MASK;
    let (hw, hh) = (w / 2.0, h / 2.0);
    let (ax, ay) = (px - x - hw, py - y - hh);
    let picked = match (ax < 0.0, ay < 0.0) {
        (true, true) => radii[0],
        (false, true) => radii[1],
        (true, false) => radii[2],
        (false, false) => radii[3],
    };
    let r = picked.min(hw.min(hh));
    let (qx, qy) = (ax.abs() - (hw - r), ay.abs() - (hh - r));
    let away = qx.max(qy).min(0.0) + (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() - r;
    -away
}

/// One draw through `program` into a cleared (or `fill`ed) `side` target,
/// read back.
fn draw_beneath(
    renderer: &mut GlesRenderer,
    side: (i32, i32),
    fill: Option<[f32; 4]>,
    (program, texture, vertices, draw, mask): (
        crate::fx0::warp_gl::BeneathProgram,
        u32,
        &[f32],
        crate::fx0::warp_gl::Draw,
        ([f32; 4], [f32; 4], [f32; 2]),
    ),
) -> Result<Vec<u8>> {
    use smithay::backend::renderer::Bind as _;
    use smithay::backend::renderer::Color32F;
    use smithay::backend::renderer::Frame as _;
    let mut pool = pool::Pool::new(0);
    let mut carrier = pool
        .carrier(renderer)
        .ok_or_else(|| anyhow!("no carrier"))?;
    let target = pool
        .target(&mut pool::Gl(renderer), side.into(), pool::Format::Rgba8)
        .ok_or_else(|| anyhow!("no pooled target"))?;
    {
        let mut bound = renderer
            .bind(&mut carrier)
            .map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        let [r, g, b, a] = fill.unwrap_or([0.0; 4]);
        frame
            .clear(
                Color32F::new(r, g, b, a),
                &[Rectangle::from_size(side.into())],
            )
            .map_err(|err| anyhow!("{err}"))?;
        let projection = *frame.projection();
        // SAFETY: inside the frame's context; the program and texture are its.
        frame
            .with_context(|gl| unsafe {
                program.draw(gl, &projection, texture, vertices, draw, mask, &[])
            })
            .map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let drawn = fx2::read_rgba(renderer, target.texture())?;
    drop(target);
    pool.sweep(renderer);
    Ok(drawn)
}

/// **Case 13i: a backdrop beneath a mesh, and its hole** (Task 54, Ruling
/// F37). The compositor's own `warp::BeneathProgram`, through a bent 2×2
/// grid of a 200×120 pane on a 640×360 target, over a "memo" whose texel
/// `(x, y)` is `(x mod 256, y mod 256, 64·⌊x/256⌋ + 32·⌊y/256⌋)`, so a flip
/// or an offset in either axis shows, mapped as `ScreenMap::monitor`
/// (640×360 at scale 1) maps it: the pixel at the grid's centre, and one a
/// quarter of the way from it to each corner (each inside the mask), is the
/// memo at its own screen position within 1, not the memo at its texture
/// coordinate; a pixel the grid puts outside the mask's rectangle is
/// transparent. Then a hole (`Draw::Erase`) over (20, 10, 160, 100) with
/// its top-left corner rounded by 16, in a grey target: transparent inside,
/// grey outside, and between on its arc. This pins the shader's convention,
/// not a rendered memo's orientation, which `dev/effects-check.sh motion`'s
/// `genie` shows against the wallpaper blurred everywhere.
fn a_backdrop_beneath_a_mesh_and_its_hole(renderer: &mut GlesRenderer) -> Result<()> {
    use crate::fx0::warp_gl::{BeneathProgram, Draw};
    println!("\n=== FX3: a backdrop beneath a mesh reads where each fragment is, and its hole ===");
    let (w, h) = MONITOR;
    let texel = |x: usize, y: usize| -> [u8; 4] {
        let byte = |n: usize| u8::try_from(n).unwrap_or(u8::MAX);
        [
            byte(x % 256),
            byte(y % 256),
            byte(64 * (x / 256) + 32 * (y / 256)),
            255,
        ]
    };
    let (memo, bytes) = fx2::upload(renderer, MONITOR, texel)?;
    // SAFETY: `with_context` makes the context current.
    let program = renderer
        .with_context(|gl| unsafe { BeneathProgram::compile(gl) })
        .map_err(|err| anyhow!("{err}"))?
        .map_err(|why| anyhow!("{why}"))?;
    let triangles = grid_triangles();
    let vertices: Vec<f32> = triangles
        .iter()
        .flat_map(|[x, y, u, v]| [*x, *y, *u, *v, 1.0])
        .collect();
    #[expect(clippy::cast_precision_loss, reason = "a monitor's side")]
    let map = [1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0];
    let side = (i32::try_from(w).unwrap_or(1), i32::try_from(h).unwrap_or(1));
    let drawn = draw_beneath(
        renderer,
        side,
        None,
        (
            program,
            memo.tex_id(),
            &vertices,
            Draw::Backdrop { map, alpha: 1.0 },
            MASK,
        ),
    )?;
    let pixel = |picture: &[u8], width: usize, (x, y): (usize, usize)| -> Vec<u8> {
        picture
            .get((y * width + x) * 4..(y * width + x) * 4 + 4)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    };
    let centre = (215.0_f32, 170.0_f32);
    let mut probes = vec![(215_usize, 170_usize)];
    for corner in [GRID[0], GRID[2], GRID[6], GRID[8]] {
        let quarter = (
            centre.0 + (corner.0 - centre.0) / 4.0,
            centre.1 + (corner.1 - centre.1) / 4.0,
        );
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a pixel on the target"
        )]
        probes.push((quarter.0 as usize, quarter.1 as usize));
    }
    for at in &probes {
        #[expect(clippy::cast_precision_loss, reason = "a pixel's centre")]
        let middle = (at.0 as f32 + 0.5, at.1 as f32 + 0.5);
        let (u, v) = uv_at(&triangles, middle).ok_or_else(|| anyhow!("{at:?} is off the grid"))?;
        let inside = inside_mask((u * MASK.2[0], v * MASK.2[1]));
        if inside < 2.0 {
            return Err(anyhow!(
                "the premise: {at:?} (uv {u:.3}, {v:.3}) is not well inside the mask ({inside:.2})"
            ));
        }
        let (got, want) = (pixel(&drawn, w, *at), pixel(&bytes, w, *at));
        if got.len() != 4 || worst(&got, &want) > 1 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a texel"
            )]
            let at_uv = pixel(
                &bytes,
                w,
                ((u * w as f32) as usize, (v * h as f32) as usize),
            );
            return Err(anyhow!(
                "beneath the mesh, {at:?} reads {got:?}, not the memo there {want:?} (the memo at its uv is {at_uv:?})"
            ));
        }
    }
    // A pixel the grid puts outside the mask's rectangle: nothing drawn.
    let outside = (111_usize, 48_usize);
    #[expect(clippy::cast_precision_loss, reason = "a pixel's centre")]
    let middle = (outside.0 as f32 + 0.5, outside.1 as f32 + 0.5);
    let (u, v) = uv_at(&triangles, middle).ok_or_else(|| anyhow!("{outside:?} is off the grid"))?;
    if inside_mask((u * MASK.2[0], v * MASK.2[1])) > -2.0 {
        return Err(anyhow!(
            "the premise: {outside:?} (uv {u:.3}, {v:.3}) is not well outside the mask"
        ));
    }
    let got = pixel(&drawn, w, outside);
    if got.iter().any(|channel| *channel > 1) {
        return Err(anyhow!(
            "outside the mask, {outside:?} reads {got:?}, not transparent"
        ));
    }
    println!(
        "  {} pixels beneath the bent grid read the memo where each is within 1; outside the mask, transparent",
        probes.len()
    );

    // The hole: a 160×100 quad at (20, 10) of a grey 200×120 target, its
    // uv 0 to 1 across it, as `warp::Mesh::quad` makes it.
    let (x0, y0, x1, y1) = (20.0_f32, 10.0_f32, 180.0_f32, 110.0_f32);
    let quad: Vec<f32> = [
        (x0, y0, 0.0, 0.0),
        (x1, y0, 1.0, 0.0),
        (x1, y1, 1.0, 1.0),
        (x0, y0, 0.0, 0.0),
        (x1, y1, 1.0, 1.0),
        (x0, y1, 0.0, 1.0),
    ]
    .iter()
    .flat_map(|(x, y, u, v)| [*x, *y, *u, *v, 1.0])
    .collect();
    let grey = 128.0 / 255.0;
    let holed = draw_beneath(
        renderer,
        (200, 120),
        Some([grey, grey, grey, 1.0]),
        (
            program,
            0,
            &quad,
            Draw::Erase,
            ([0.0, 0.0, 160.0, 100.0], MASK.1, [160.0, 100.0]),
        ),
    )?;
    let inside = pixel(&holed, 200, (100, 60));
    if inside.iter().any(|channel| *channel > 1) {
        return Err(anyhow!(
            "inside the hole (100, 60) reads {inside:?}, not transparent"
        ));
    }
    let beside = pixel(&holed, 200, (5, 5));
    if beside.len() != 4 || worst(&beside, &[128, 128, 128, 255]) > 1 {
        return Err(anyhow!(
            "beside the hole (5, 5) reads {beside:?}, not the grey"
        ));
    }
    // On the top-left corner's diagonal, where the arc of radius 16 about
    // (36, 26) crosses: partly erased.
    let arc = pixel(&holed, 200, (24, 14));
    if !arc.first().is_some_and(|red| (8..120).contains(red)) {
        return Err(anyhow!(
            "on the hole's rounded corner (24, 14) reads {arc:?}, not between"
        ));
    }
    println!("  the hole: transparent inside, grey beside it, {arc:?} on its rounded corner");
    Ok(())
}
