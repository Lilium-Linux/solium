//! Normalise a screencopy buffer as a recording client does: row storage first,
//! then the advertised output transform. The result is tightly packed BGRX.

pub(super) fn normalise(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    flags: u32,
    transform: u32,
) -> Result<(u32, u32, u32, Vec<u8>), String> {
    if width == 0 || height == 0 || transform > 7 || stride < width.saturating_mul(4) {
        return Err("invalid capture geometry or transform".to_owned());
    }
    let (w, h) = if transform % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let packed = w.checked_mul(4).ok_or("capture stride overflow")?;
    let len =
        usize::try_from(u64::from(packed) * u64::from(h)).map_err(|_| "capture size overflow")?;
    let mut result = vec![0; len];
    for y in 0..h {
        for x in 0..w {
            let (sx, mut sy) = match transform {
                0 => (x, y),
                1 => (y, height - 1 - x),
                2 => (width - 1 - x, height - 1 - y),
                3 => (width - 1 - y, x),
                4 => (width - 1 - x, y),
                5 => (y, x),
                6 => (x, height - 1 - y),
                7 => (width - 1 - y, height - 1 - x),
                _ => unreachable!(),
            };
            if flags & 1 != 0 {
                sy = height - 1 - sy;
            }
            let source = sy as usize * stride as usize + sx as usize * 4;
            let dest = y as usize * packed as usize + x as usize * 4;
            result[dest..dest + 4]
                .copy_from_slice(pixels.get(source..source + 4).ok_or("short capture row")?);
        }
    }
    Ok((w, h, packed, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_transforms_restore_the_same_asymmetric_picture() {
        let expected: Vec<u8> = [1, 2, 3, 4, 5, 6]
            .into_iter()
            .flat_map(|v| [v, 0, 0, 255])
            .collect();
        for (transform, w, h, values) in [
            (0, 3, 2, [1, 2, 3, 4, 5, 6]),
            (1, 2, 3, [3, 6, 2, 5, 1, 4]),
            (2, 3, 2, [6, 5, 4, 3, 2, 1]),
            (3, 2, 3, [4, 1, 5, 2, 6, 3]),
            (4, 3, 2, [3, 2, 1, 6, 5, 4]),
            (5, 2, 3, [1, 4, 2, 5, 3, 6]),
            (6, 3, 2, [4, 5, 6, 1, 2, 3]),
            (7, 2, 3, [6, 3, 5, 2, 4, 1]),
        ] {
            let pixels: Vec<u8> = values.into_iter().flat_map(|v| [v, 0, 0, 255]).collect();
            let actual = normalise(&pixels, w, h, w * 4, 0, transform).expect("normalise fixture");
            assert_eq!(
                actual,
                (3, 2, 12, expected.clone()),
                "transform {transform}"
            );
        }
    }
    #[test]
    fn inverted_row_storage_and_padding_are_independent_of_output_transform() {
        let pixels = [
            4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255, 99, 99, 99, 99, 1, 0, 0, 255, 2, 0, 0, 255,
            3, 0, 0, 255, 99, 99, 99, 99,
        ];
        let expected: Vec<u8> = [1, 2, 3, 4, 5, 6]
            .into_iter()
            .flat_map(|v| [v, 0, 0, 255])
            .collect();
        assert_eq!(
            normalise(&pixels, 3, 2, 16, 1, 0).unwrap(),
            (3, 2, 12, expected)
        );
    }
}
