use super::*;

/// A `width` by `height` image whose red runs from 0 at the left to 255 at
/// the right, opaque.
fn ramp(width: u32, height: u32) -> Handle {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..height {
        for x in 0..width {
            let red = (x * 255 / (width - 1)) as u8;
            pixels.extend_from_slice(&[red, 0, 0, 255]);
        }
    }
    Handle::from_rgba(width, height, pixels)
}

fn pixels(handle: &Handle) -> (u32, u32, Vec<u8>) {
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        panic!("pixels");
    };
    (*width, *height, pixels.to_vec())
}

fn at(image: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.0 + x) * 4) as usize;
    image.2[index..index + 4].try_into().unwrap()
}

#[test]
fn a_tilt_is_narrower_and_keeps_the_height() {
    for tilt in [Tilt::Left, Tilt::Right] {
        let warped = pixels(&warp(&ramp(100, 150), tilt).expect("warped"));
        assert_eq!((warped.0, warped.1), (60, 150), "{tilt:?}");
    }
    assert!(warp(&ramp(100, 150), Tilt::None).is_none());
}

#[test]
fn only_the_posters_two_by_three_middle_is_warped() {
    // A wide source keeps its middle 100 columns, a tall one its middle
    // 150 rows, as a flat cover crops them.
    let wide = pixels(&warp(&ramp(200, 150), Tilt::Left).unwrap());
    assert_eq!((wide.0, wide.1), (60, 150));
    // The ramp's ends were cropped away: no near-black or near-white.
    let row: Vec<u8> = (0..60).map(|x| at(&wide, x, 75)[0]).collect();
    assert!(row.iter().all(|red| (50..=205).contains(red)), "{row:?}");
    let tall = pixels(&warp(&ramp(100, 300), Tilt::Right).unwrap());
    assert_eq!((tall.0, tall.1), (60, 150));
}

#[test]
fn the_far_edge_is_shorter_and_the_near_edge_full_height() {
    let left = pixels(&warp(&ramp(100, 150), Tilt::Left).unwrap());
    // The far (left) edge leaves its top and bottom 10% clear.
    assert_eq!(at(&left, 0, 5)[3], 0);
    assert_eq!(at(&left, 0, 144)[3], 0);
    assert_eq!(at(&left, 0, 75)[3], 255);
    // The near (right) edge runs top to bottom, its corner pixels only
    // trimmed by the slant's last sliver.
    assert!(at(&left, 59, 0)[3] > 200);
    assert!(at(&left, 59, 149)[3] > 200);
    let right = pixels(&warp(&ramp(100, 150), Tilt::Right).unwrap());
    assert_eq!(at(&right, 59, 5)[3], 0);
    assert_eq!(at(&right, 59, 144)[3], 0);
    assert!(at(&right, 0, 0)[3] > 200);
    assert!(at(&right, 0, 149)[3] > 200);
}

#[test]
fn the_slanted_edges_are_antialiased() {
    let left = pixels(&warp(&ramp(100, 150), Tilt::Left).unwrap());
    // Down the column a third of the way in, alpha climbs through partial
    // values at the edge rather than jumping.
    let column: Vec<u8> = (0..15).map(|y| at(&left, 20, y)[3]).collect();
    assert!(
        column.iter().any(|alpha| *alpha > 0 && *alpha < 255),
        "{column:?}"
    );
    assert!(
        column.windows(2).all(|pair| pair[0] <= pair[1]),
        "{column:?}"
    );
}

#[test]
fn the_image_keeps_its_left_and_right_and_its_far_half_narrows() {
    for tilt in [Tilt::Left, Tilt::Right] {
        let warped = pixels(&warp(&ramp(100, 150), tilt).unwrap());
        let row = 75;
        assert!(at(&warped, 0, row)[0] < 20, "{tilt:?}");
        assert!(at(&warped, 59, row)[0] > 235, "{tilt:?}");
        // Where the source's middle column lands: toward the far edge.
        let middle = (0..60).find(|&x| at(&warped, x, row)[0] >= 128).unwrap();
        match tilt {
            Tilt::Left => assert!(middle < 30, "{middle}"),
            _ => assert!(middle > 30, "{middle}"),
        }
    }
}

#[test]
fn the_top_inset_runs_from_the_far_edge_to_nothing() {
    assert_eq!(top_inset(Tilt::Left, 0.0), INSET);
    assert_eq!(top_inset(Tilt::Left, 1.0), 0.0);
    assert_eq!(top_inset(Tilt::Right, 0.0), 0.0);
    assert_eq!(top_inset(Tilt::Right, 1.0), INSET);
    assert_eq!(top_inset(Tilt::None, 0.3), 0.0);
}
