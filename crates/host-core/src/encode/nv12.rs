//! BGRA to NV12 conversion for encoders that only accept YUV input (many
//! hardware Media Foundation encoders).
//!
//! BT.601 limited range with integer coefficients, the colour space a
//! decoder assumes for a stream without colour description (openh264 uses
//! the same). Chroma is the average of each 2x2 block.

/// Size in bytes of an NV12 image: a full-resolution Y plane followed by a
/// half-resolution interleaved UV plane, both with stride `width`.
pub(crate) fn nv12_len(width: usize, height: usize) -> usize {
    width * height * 3 / 2
}

/// Converts a tightly packed BGRA image into `nv12` (see [`nv12_len`]).
///
/// Returns `false` and writes nothing if the dimensions are odd or zero, or
/// the buffers do not have the exact sizes for `width` x `height`.
pub(crate) fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, nv12: &mut [u8]) -> bool {
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || bgra.len() != width * height * 4
        || nv12.len() != nv12_len(width, height)
    {
        return false;
    }
    let (y_plane, uv_plane) = nv12.split_at_mut(width * height);
    let row_bytes = width * 4;
    for (pair, (y_rows, uv_row)) in bgra.chunks_exact(row_bytes * 2).zip(
        y_plane
            .chunks_exact_mut(width * 2)
            .zip(uv_plane.chunks_exact_mut(width)),
    ) {
        let (top, bottom) = pair.split_at(row_bytes);
        let (y_top, y_bottom) = y_rows.split_at_mut(width);
        // Lengths are exact multiples (checked above), so no remainders.
        for ((((top_px, bottom_px), y_top), y_bottom), uv) in top
            .as_chunks::<8>()
            .0
            .iter()
            .zip(bottom.as_chunks::<8>().0)
            .zip(y_top.as_chunks_mut::<2>().0)
            .zip(y_bottom.as_chunks_mut::<2>().0)
            .zip(uv_row.as_chunks_mut::<2>().0)
        {
            let px = |row: &[u8; 8], i: usize| {
                (
                    i32::from(row[i * 4 + 2]),
                    i32::from(row[i * 4 + 1]),
                    i32::from(row[i * 4]),
                )
            };
            let block = [
                px(top_px, 0),
                px(top_px, 1),
                px(bottom_px, 0),
                px(bottom_px, 1),
            ];
            y_top[0] = luma(block[0]);
            y_top[1] = luma(block[1]);
            y_bottom[0] = luma(block[2]);
            y_bottom[1] = luma(block[3]);
            let (r, g, b) = block.iter().fold((0, 0, 0), |(r, g, b), &(pr, pg, pb)| {
                (r + pr, g + pg, b + pb)
            });
            // Rounded average of the four pixels.
            let average = ((r + 2) >> 2, (g + 2) >> 2, (b + 2) >> 2);
            uv[0] = chroma_u(average);
            uv[1] = chroma_v(average);
        }
    }
    true
}

/// The results are within 0..=255 for any 8-bit input, so `as u8` is exact.
fn luma((r, g, b): (i32, i32, i32)) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

fn chroma_u((r, g, b): (i32, i32, i32)) -> u8 {
    (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8
}

fn chroma_v((r, g, b): (i32, i32, i32)) -> u8 {
    (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` x `height` BGRA image of one colour, given as (r, g, b).
    fn solid(width: usize, height: usize, (r, g, b): (u8, u8, u8)) -> Vec<u8> {
        [b, g, r, 255].repeat(width * height)
    }

    fn convert(bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut nv12 = vec![0xEE; nv12_len(width, height)];
        assert!(bgra_to_nv12(bgra, width, height, &mut nv12));
        nv12
    }

    #[test]
    fn solid_colours_match_bt601_limited_range() {
        // (r, g, b) -> (y, u, v)
        for (rgb, yuv) in [
            ((0, 0, 0), (16, 128, 128)),
            ((255, 255, 255), (235, 128, 128)),
            ((255, 0, 0), (82, 90, 240)),
            ((0, 255, 0), (144, 54, 34)),
            ((0, 0, 255), (41, 240, 110)),
        ] {
            let nv12 = convert(&solid(4, 2, rgb), 4, 2);
            let (y, u, v) = yuv;
            assert_eq!(nv12, [y, y, y, y, y, y, y, y, u, v, u, v], "rgb {rgb:?}");
        }
    }

    #[test]
    fn luma_is_per_pixel_and_chroma_is_averaged_per_block() {
        // 2x2 block: white, black / black, white. Then a solid red block.
        let (white, black, red) = ([255u8; 4], [0, 0, 0, 255], [0, 0, 255, 255]);
        let bgra = [white, black, red, red, black, white, red, red].concat();
        let nv12 = convert(&bgra, 4, 2);
        assert_eq!(&nv12[..8], [235, 16, 82, 82, 16, 235, 82, 82]);
        // Average of the first block is (128, 128, 128) (rounded): grey.
        assert_eq!(&nv12[8..], [128, 128, 90, 240]);
    }

    #[test]
    fn planes_have_the_documented_layout() {
        // 2x4: two vertically stacked 2x2 blocks, black over white.
        let bgra = [solid(2, 2, (0, 0, 0)), solid(2, 2, (255, 255, 255))].concat();
        let nv12 = convert(&bgra, 2, 4);
        assert_eq!(nv12.len(), 12);
        assert_eq!(&nv12[..8], [16, 16, 16, 16, 235, 235, 235, 235]);
        assert_eq!(&nv12[8..], [128, 128, 128, 128]);
    }

    #[test]
    fn rejects_odd_dimensions_and_wrong_sizes() {
        let mut nv12 = vec![0xEE; nv12_len(4, 2)];
        assert!(!bgra_to_nv12(&solid(3, 2, (0, 0, 0)), 3, 2, &mut nv12));
        assert!(!bgra_to_nv12(&solid(4, 1, (0, 0, 0)), 4, 1, &mut nv12));
        assert!(!bgra_to_nv12(&solid(4, 4, (0, 0, 0)), 4, 2, &mut nv12));
        assert!(!bgra_to_nv12(&solid(4, 2, (0, 0, 0)), 4, 2, &mut nv12[1..]));
        assert!(nv12.iter().all(|&byte| byte == 0xEE), "nothing written");
    }
}
