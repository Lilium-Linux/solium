//! Compare independently requested crops with the matching full-output pixels.

use super::*;

impl Captured {
    fn rgb(&self, x: u32, y: u32) -> Result<[u8; 3], String> {
        if x >= self.width || y >= self.height {
            return Err(format!(
                "pixel ({x},{y}) is outside {}x{}",
                self.width, self.height
            ));
        }
        let row = if self.flags & 1 == 0 {
            y
        } else {
            self.height - 1 - y
        };
        let at = row as usize * self.stride as usize + x as usize * 4;
        let bgr = self.pixels.get(at..at + 3).ok_or("a short capture row")?;
        Ok([bgr[2], bgr[1], bgr[0]])
    }

    fn save(&self, name: &str) -> Result<(), String> {
        let Some(folder) = std::env::var_os("WL_PROBE_CROP_DIR") else {
            return Ok(());
        };
        let mut bytes = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for y in 0..self.height {
            for x in 0..self.width {
                bytes.extend_from_slice(&self.rgb(x, y)?);
            }
        }
        std::fs::write(std::path::PathBuf::from(folder).join(name), bytes)
            .map_err(|err| format!("saving capture: {err}"))
    }
}

pub(super) fn check(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    anchor_a_bar(connection, queue, probe)?;
    for index in 0..probe.outputs.len() {
        let screen = &probe.outputs[index];
        let name = screen.name.clone();
        // An explicit fractional fixture scale, otherwise wl_output's integer
        // scale. This probe is a controlled fixture, not a scale detector.
        let scale = match std::env::var("WL_PROBE_CROP_SCALE") {
            Ok(value) => value.parse::<f64>().map_err(|err| err.to_string())?,
            Err(_) => f64::from(screen.scale.max(1)),
        };
        if !scale.is_finite() || scale <= 0.0 {
            return Err("the fixture scale must be positive and finite".to_owned());
        }
        let full = capture_output(connection, queue, probe, index, None)?;
        full.save(&format!("{name}-full.ppm"))?;
        // Literal fixture colours prevent equally blank or mirrored pictures
        // from satisfying the crop/full comparison.
        let base = if name.ends_with("-2") {
            [64, 33, 57]
        } else {
            [25, 37, 50]
        };
        for (x, y, expected) in [
            (20, 20, [32, 80, 128]),
            (30, 80, [233, 72, 54]),
            (150, 110, [49, 186, 102]),
            (250, 190, [231, 197, 65]),
            (350, 300, base),
        ] {
            let px = (f64::from(x) * scale).round() as u32;
            let py = (f64::from(y) * scale).round() as u32;
            let actual = full.rgb(px, py)?;
            if actual != expected {
                return Err(format!(
                    "{name} fixture at ({x},{y}): {actual:?}, expected {expected:?}"
                ));
            }
        }
        let logical_w = (f64::from(full.width) / scale).round() as i32;
        let logical_h = (f64::from(full.height) / scale).round() as i32;
        for (label, region) in [
            ("bar-and-scene", (37, 11, 251, 211)),
            ("interior", (53, 79, 197, 113)),
            ("edge", (logical_w - 9, logical_h - 7, 9, 7)),
        ] {
            let crop = capture_output(connection, queue, probe, index, Some(region))?;
            crop.save(&format!("{name}-{label}.ppm"))?;
            let (x, y, w, h) = region;
            let left = (f64::from(x) * scale).round() as u32;
            let top = (f64::from(y) * scale).round() as u32;
            let right = (f64::from(x + w) * scale).round() as u32;
            let bottom = (f64::from(y + h) * scale).round() as u32;
            if (crop.width, crop.height) != (right - left, bottom - top) {
                return Err(format!(
                    "{name}/{label}: {}x{}, expected {}x{}",
                    crop.width,
                    crop.height,
                    right - left,
                    bottom - top
                ));
            }
            for cy in 0..crop.height {
                for cx in 0..crop.width {
                    let expected = full.rgb(left + cx, top + cy)?;
                    let actual = crop.rgb(cx, cy)?;
                    if actual != expected {
                        return Err(format!(
                            "{name}/{label}: crop mismatch at ({cx},{cy}): {actual:?}, full frame {expected:?}"
                        ));
                    }
                }
            }
            println!(
                "PASS {name}/{label}: {} pixels match the full output at scale {scale}",
                crop.width * crop.height
            );
        }
    }
    Ok(())
}
