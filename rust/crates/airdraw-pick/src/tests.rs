use super::*;
use airdraw_doc::BrushRef;

const HW: f32 = 0.005;

/// At (0, 0, 5) looking down −Z, 60° tall view, 1000×1000 px: the origin is at the centre and
/// a metre at depth 5 is `PX` pixels.
fn cam() -> Camera {
    let proj = Mat4::perspective_rh(60f32.to_radians(), 1.0, 0.05, 100.0);
    Camera::new(Affine3A::from_translation(Vec3::new(0.0, 0.0, -5.0)), proj, Vec2::splat(1000.0))
}

const PX: f32 = 173.205_08;

fn line(id: u64, layer: u32, from: Vec3, to: Vec3, n: usize) -> Stroke {
    let points = (0..n)
        .map(|i| {
            let f = i as f32 / (n - 1).max(1) as f32;
            InputPoint { pos: from.lerp(to, f), normal: Vec3::Z, pressure: f, tilt: Some(1.0 - f), t: 2.0 * f as f64 }
        })
        .collect::<Vec<_>>();
    Stroke { id: StrokeId(id), layer: LayerId(layer), brush: BrushRef(0), points: points.into() }
}

fn x_line(id: u64, y: f32, z: f32, half: f32) -> Stroke {
    line(id, 1, Vec3::new(-half, y, z), Vec3::new(half, y, z), 101)
}

fn index(strokes: &[Stroke]) -> PickIndex {
    let mut i = PickIndex::new();
    for s in strokes {
        i.insert(s, HW);
    }
    i
}

fn ids(v: &[u64]) -> Vec<StrokeId> {
    v.iter().map(|&i| StrokeId(i)).collect()
}

#[test]
fn projection_matches_the_camera() {
    let c = cam();
    let (at, depth) = c.project(Vec3::ZERO).unwrap();
    assert!(at.distance(Vec2::splat(500.0)) < 1e-3 && (depth - 5.0).abs() < 1e-5);
    let (at, _) = c.project(Vec3::new(1.0, 1.0, 0.0)).unwrap();
    assert!(at.distance(Vec2::new(500.0 + PX, 500.0 - PX)) < 1e-2, "{at}: y is down");
    assert!(c.project(Vec3::new(0.0, 0.0, 6.0)).is_none(), "behind the camera");
}

#[test]
fn eraser_hits_only_what_it_touches() {
    let i =
        index(&[x_line(1, 0.0, 0.0, 1.0), x_line(2, 1.0, 0.0, 1.0), line(3, 1, Vec3::new(0.3, -0.3, 4.0), Vec3::new(0.3, -0.3, 6.0), 9)]);
    let c = cam();
    assert_eq!(i.under_capsule(&c, Vec2::new(500.0, 505.0), Vec2::new(500.0, 505.0), 10.0, true), ids(&[1]));
    assert!(i.under_capsule(&c, Vec2::new(500.0, 400.0), Vec2::new(500.0, 400.0), 10.0, false).is_empty());
    // The stroke width counts: 9 px away with a 8 px eraser still touches a 0.87 px half width.
    let reach = 500.0 + 8.0 + HW * PX - 0.01;
    assert_eq!(i.under_capsule(&c, Vec2::new(500.0, reach), Vec2::new(500.0, reach), 8.0, false), ids(&[1]));
}

#[test]
fn a_fast_swipe_cant_skip_a_line() {
    let i = index(&[line(1, 1, Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 50)]);
    let c = cam();
    let (a, b) = (Vec2::new(300.0, 500.0), Vec2::new(700.0, 500.0));
    assert!(i.under_capsule(&c, a, a, 5.0, true).is_empty() && i.under_capsule(&c, b, b, 5.0, true).is_empty());
    assert_eq!(i.under_capsule(&c, a, b, 5.0, true), ids(&[1]));
}

#[test]
fn front_only_spares_strokes_behind() {
    // 1 in front, 2 two metres behind it on the same screen line, 3 crossing 1 at its depth.
    let behind = x_line(2, 0.0, -2.0, 2.0);
    let crossing = line(3, 1, Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 101);
    let i = index(&[x_line(1, 0.0, 0.0, 1.0), behind, crossing]);
    let c = cam();
    let p = Vec2::splat(500.0);
    assert_eq!(i.under_capsule(&c, p, p, 10.0, false), ids(&[1, 2, 3]));
    assert_eq!(i.under_capsule(&c, p, p, 10.0, true), ids(&[1, 3]), "same depth both go, the one behind stays");
    // Past the end of 1 nothing is in front of 2.
    let q = Vec2::new(720.0, 500.0);
    assert_eq!(i.under_capsule(&c, q, q, 10.0, true), ids(&[2]));
    // A swipe along the line: 2 is behind 1 everywhere except beyond 1's ends.
    let (a, b) = (Vec2::new(400.0, 500.0), Vec2::new(600.0, 500.0));
    assert_eq!(i.under_capsule(&c, a, b, 10.0, true), ids(&[1, 3]));
    assert_eq!(i.under_capsule(&c, a, Vec2::new(740.0, 500.0), 10.0, true), ids(&[1, 2, 3]));
}

#[test]
fn a_line_behind_is_spared_only_where_the_front_one_is_under_the_eraser() {
    // Vertical lines: 1 at screen x 500, 2 two metres behind it at screen x 525.
    let x = 25.0 / (PX * 5.0 / 7.0);
    let back = line(2, 1, Vec3::new(x, -2.0, -2.0), Vec3::new(x, 2.0, -2.0), 101);
    let i = index(&[line(1, 1, Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 101), back]);
    let c = cam();
    let p = Vec2::new(512.0, 500.0);
    assert_eq!(i.under_capsule(&c, p, p, 20.0, true), ids(&[1]), "both under the eraser: the front one goes");
    // Swiping across, the eraser also passes over 2 where 1 isn't under it.
    assert_eq!(i.under_capsule(&c, Vec2::new(300.0, 500.0), Vec2::new(700.0, 500.0), 20.0, true), ids(&[1, 2]));
    assert_eq!(i.under_capsule(&c, Vec2::new(300.0, 500.0), Vec2::new(520.0, 500.0), 20.0, true), ids(&[1]), "stopping short spares it");
}

#[test]
fn a_stroke_grazing_the_eraser_between_positions_still_counts() {
    // Positions are every 5 px; a zero-width dot 19.9 px off the path halfway between two of
    // them is 20.06 px from both, outside the 20 px eraser at each.
    let mut i = PickIndex::new();
    let at = Vec3::new(2.5 / PX, -19.9 / PX, 0.0);
    i.insert(&line(1, 1, at, at, 1), 0.0);
    let (a, b) = (Vec2::splat(500.0), Vec2::new(540.0, 500.0));
    assert_eq!(i.under_capsule(&cam(), a, b, 20.0, true), ids(&[1]));
}

#[test]
fn skipped_layers_and_removed_strokes_arent_picked() {
    let mut s2 = x_line(2, 0.0, 0.0, 1.0);
    s2.layer = LayerId(2);
    let mut i = index(&[x_line(1, 0.0, 0.0, 1.0), s2, x_line(3, 0.0, 0.0, 1.0)]);
    let (c, p) = (cam(), Vec2::splat(500.0));
    i.set_skipped_layers([LayerId(2)]);
    assert_eq!(i.under_capsule(&c, p, p, 5.0, false), ids(&[1, 3]));
    i.remove(StrokeId(1));
    assert!(!i.contains(StrokeId(1)) && i.contains(StrokeId(3)) && i.len() == 2);
    assert_eq!(i.under_capsule(&c, p, p, 5.0, false), ids(&[3]));
    i.set_skipped_layers([]);
    i.remove(StrokeId(3));
    assert_eq!(i.under_capsule(&c, p, p, 5.0, false), ids(&[2]));
    // Re-inserting replaces: stroke 2 moved away.
    i.insert(&x_line(2, 1.0, 0.0, 1.0), HW);
    assert_eq!(i.len(), 1);
    assert!(i.under_capsule(&c, p, p, 5.0, false).is_empty());
    i.clear();
    assert!(i.is_empty());
}

#[test]
fn a_tap_picks_the_front_stroke_then_the_closest() {
    let i = index(&[x_line(1, 0.0, 0.0, 1.0), x_line(2, 0.0, -2.0, 2.0), x_line(4, 0.03, 0.0, 1.0)]);
    let c = cam();
    assert_eq!(i.nearest(&c, Vec2::splat(500.0), 10.0), Some(StrokeId(1)));
    assert_eq!(i.nearest(&c, Vec2::new(720.0, 500.0), 10.0), Some(StrokeId(2)));
    // 4 is 5 px above 1 at the same depth: the tap closer to it wins.
    assert_eq!(i.nearest(&c, Vec2::new(500.0, 500.0 - 0.03 * PX - 1.0), 10.0), Some(StrokeId(4)));
    assert_eq!(i.nearest(&c, Vec2::new(500.0, 450.0), 10.0), None);
    // A stroke behind wins over a nearer-on-screen one only if it's about as near in depth.
    let y = -5.0 / (PX * 5.0 / 7.0);
    let i = index(&[x_line(1, 0.0, 0.0, 1.0), x_line(2, y, -2.0, 2.0)]);
    assert_eq!(i.nearest(&c, Vec2::new(500.0, 504.0), 10.0), Some(StrokeId(1)));
}

#[test]
fn lasso_takes_strokes_mostly_inside_at_any_depth() {
    // Screen x 400..800 around the middle row.
    let lasso = [Vec2::new(400.0, 450.0), Vec2::new(800.0, 450.0), Vec2::new(800.0, 550.0), Vec2::new(400.0, 550.0)];
    let mostly = x_line(1, 0.0, 0.0, 1.0); // 79% inside
    let behind = x_line(2, 0.0, -2.0, 2.0); // 70% inside, two metres back
    let little = line(3, 1, Vec3::new(-1.0, 0.1, 0.0), Vec3::new(-0.3, 0.1, 0.0), 101); // 40% inside
    let outside = x_line(4, 1.0, 0.0, 1.0);
    let i = index(&[mostly, behind, little, outside]);
    assert_eq!(i.in_lasso(&cam(), &lasso), ids(&[1, 2]));
    assert!(i.in_lasso(&cam(), &lasso[..2]).is_empty(), "not a polygon");
}

/// The 21-point line from x −1 to 1, 0.1 m apart.
fn short_line() -> Stroke {
    line(1, 1, Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 21)
}

#[test]
fn cutting_the_middle_leaves_two_pieces_ending_at_the_eraser() {
    let s = short_line();
    let c = cam();
    let centre = Vec2::splat(500.0);
    let pieces = cut(&c, &s.points, HW, &[(centre, centre)], 20.0, 0.001).unwrap();
    assert_eq!(pieces.len(), 2);
    let reach = 20.0 + HW * PX;
    let edge = reach / PX;
    let (left, right) = (&pieces[0], &pieces[1]);
    assert_eq!(left[0], s.points[0], "untouched points are exactly the originals");
    assert_eq!(&left[..9], &s.points[..9]);
    assert_eq!(&right[1..], &s.points[12..]);
    let end = left.last().unwrap();
    assert!((end.pos.x + edge).abs() < 1e-4 && (right[0].pos.x - edge).abs() < 1e-4);
    assert!((c.project(end.pos).unwrap().0.distance(centre) - reach).abs() < 0.02, "the cut is at the eraser's edge");
    // Pressure, tilt and time are interpolated between the neighbours (x −0.2 and −0.1).
    let f = (-edge + 0.2) / 0.1;
    let (a, b) = (&s.points[8], &s.points[9]);
    assert!((end.pressure - (a.pressure + (b.pressure - a.pressure) * f)).abs() < 1e-5);
    assert!((end.tilt.unwrap() - (a.tilt.unwrap() + (b.tilt.unwrap() - a.tilt.unwrap()) * f)).abs() < 1e-5);
    assert!((end.t - (a.t + (b.t - a.t) * f as f64)).abs() < 1e-5);
    assert_eq!(end.normal, Vec3::Z);
}

#[test]
fn a_drag_across_cuts_like_the_eraser_swept_there() {
    let s = short_line();
    let c = cam();
    let dot = cut(&c, &s.points, HW, &[(Vec2::splat(500.0), Vec2::splat(500.0))], 20.0, 0.001).unwrap();
    let drag = cut(&c, &s.points, HW, &[(Vec2::new(500.0, 300.0), Vec2::new(500.0, 700.0))], 20.0, 0.001).unwrap();
    assert_eq!(drag.len(), 2);
    for (x, y) in dot.iter().flatten().zip(drag.iter().flatten()) {
        assert!(x.pos.distance(y.pos) < 1e-4);
    }
    // Several moves that overlap make one gap; two apart make three pieces.
    let near = |x: f32| (Vec2::new(x, 500.0), Vec2::new(x, 500.0));
    assert_eq!(cut(&c, &s.points, HW, &[near(490.0), near(510.0)], 20.0, 0.001).unwrap().len(), 2);
    assert_eq!(cut(&c, &s.points, HW, &[near(400.0), near(600.0)], 20.0, 0.001).unwrap().len(), 3);
    // A piece shorter than the point spacing between two erasers survives, interpolated.
    let off = 20.0 + HW * PX + 0.03 * PX;
    let pieces = cut(&c, &s.points, HW, &[near(500.0 - off), near(500.0 + off)], 20.0, 0.001).unwrap();
    assert_eq!(pieces.len(), 3);
    assert!(pieces[1].len() == 3 && (pieces[1][0].pos.x + 0.03).abs() < 1e-4 && pieces[1][1] == s.points[10]);
}

#[test]
fn untouched_covered_end_and_tiny_pieces() {
    let s = short_line();
    let c = cam();
    let at = |x: f32| [(Vec2::new(x, 500.0), Vec2::new(x, 500.0))];
    assert_eq!(cut(&c, &s.points, HW, &[(Vec2::new(500.0, 600.0), Vec2::new(500.0, 600.0))], 5.0, 0.001), None, "not touched");
    assert_eq!(cut(&c, &s.points, HW, &[(Vec2::new(200.0, 500.0), Vec2::new(800.0, 500.0))], 20.0, 0.001), Some(vec![]), "all gone");
    // Erasing the left end leaves one piece that keeps the right end exactly.
    let one = cut(&c, &s.points, HW, &at(500.0 - PX), 20.0, 0.001).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].last(), s.points.last());
    // An eraser just short of the right end leaves a 2 mm stub: dropped below min_len.
    let stub = 500.0 + PX - (20.0 + HW * PX) - 0.002 * PX;
    let pieces = cut(&c, &s.points, HW, &at(stub), 20.0, 0.01).unwrap();
    assert_eq!(pieces.len(), 1, "the stub is gone, the long left piece stays");
    assert!(pieces[0][0] == s.points[0]);
    assert_eq!(cut(&c, &s.points, HW, &at(stub), 20.0, 0.001).unwrap().len(), 2, "kept with a smaller minimum");
}

#[test]
fn single_point_strokes() {
    let dot = line(1, 1, Vec3::ZERO, Vec3::ZERO, 1);
    let c = cam();
    let p = Vec2::splat(500.0);
    assert_eq!(cut(&c, &dot.points, HW, &[(p, p)], 5.0, 0.0), Some(vec![]));
    assert_eq!(cut(&c, &dot.points, HW, &[(p + Vec2::X * 9.0, p + Vec2::X * 9.0)], 5.0, 0.0), None);
    let i = index(&[dot]);
    assert_eq!(i.under_capsule(&c, p, p, 5.0, true), ids(&[1]));
    assert_eq!(i.nearest(&c, p + Vec2::X * 3.0, 5.0), Some(StrokeId(1)));
}

#[test]
fn strokes_through_the_camera_dont_break_picking() {
    let through = line(1, 1, Vec3::new(0.0, 0.0, 4.0), Vec3::new(0.0, 0.0, 6.0), 9);
    let i = index(std::slice::from_ref(&through));
    let c = cam();
    let p = Vec2::splat(500.0);
    let _ = i.under_capsule(&c, p, p, 10.0, true);
    let _ = i.nearest(&c, p, 10.0);
    let _ = cut(&c, &through.points, HW, &[(p, p)], 10.0, 0.0);
    assert!(i.in_lasso(&c, &[Vec2::ZERO, Vec2::new(1000.0, 0.0), Vec2::splat(1000.0)]).is_empty());
}

#[test]
fn segment_helpers() {
    let (s, t, d) = closest_segments(Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 3.0));
    assert!((s - 0.5).abs() < 1e-6 && t.abs() < 1e-6 && (d - 1.0).abs() < 1e-6);
    let (_, _, d) = closest_segments(Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0));
    assert!(d.abs() < 1e-6, "crossing");
    // A segment through a capsule's side, through one cap, and past it.
    let r = segment_in_capsule(Vec2::new(5.0, -10.0), Vec2::new(5.0, 10.0), Vec2::ZERO, Vec2::new(10.0, 0.0), 2.0).unwrap();
    assert!((r.0 - 0.4).abs() < 1e-5 && (r.1 - 0.6).abs() < 1e-5);
    let r = segment_in_capsule(Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0), Vec2::ZERO, Vec2::new(5.0, 0.0), 1.0).unwrap();
    assert!((r.0 - 0.45).abs() < 1e-5 && (r.1 - 0.8).abs() < 1e-5);
    assert!(segment_in_capsule(Vec2::new(-10.0, 5.0), Vec2::new(10.0, 5.0), Vec2::ZERO, Vec2::new(5.0, 0.0), 1.0).is_none());
    let square = [Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::ONE, Vec2::new(0.0, 1.0)];
    assert!(inside(&square, Vec2::splat(0.5)) && !inside(&square, Vec2::new(1.5, 0.5)));
}

/// Timing on a 1M-point sketch: 1,000 wavy strokes of 1,000 points filling the view.
/// `cargo test --release -p airdraw-pick -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_on_a_million_points() {
    let mut seed = 1u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    };
    let mut i = PickIndex::new();
    for id in 0..1000 {
        let (start, dir) = (Vec3::new(rnd() * 1.4, rnd() * 1.4, rnd()), Vec3::new(rnd(), rnd(), rnd() * 0.3).normalize() * 0.0006);
        let points: Vec<InputPoint> = (0..1000)
            .map(|k| InputPoint {
                pos: start + dir * k as f32 + Vec3::Y * (k as f32 * 0.02).sin() * 0.02,
                normal: Vec3::Z,
                pressure: 0.5,
                tilt: None,
                t: k as f64,
            })
            .collect();
        i.insert(&Stroke { id: StrokeId(id), layer: LayerId(1), brush: BrushRef(0), points: points.into() }, HW);
    }
    let c = cam();
    let moves: Vec<(Vec2, Vec2)> =
        (0..200).map(|k| (Vec2::new(100.0 + k as f32 * 4.0, 500.0), Vec2::new(104.0 + k as f32 * 4.0, 503.0))).collect();
    let t = std::time::Instant::now();
    let mut hits = 0;
    for &(a, b) in &moves {
        hits += i.under_capsule(&c, a, b, 24.0, true).len();
    }
    let per_move = t.elapsed().as_secs_f64() * 1000.0 / moves.len() as f64;
    let t = std::time::Instant::now();
    let lasso = i.in_lasso(&c, &[Vec2::new(300.0, 300.0), Vec2::new(700.0, 300.0), Vec2::new(700.0, 700.0), Vec2::new(300.0, 700.0)]).len();
    let lasso_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    let tap = i.nearest(&c, Vec2::splat(500.0), 12.0);
    println!(
        "eraser move {per_move:.3} ms ({hits} hits), lasso {lasso_ms:.1} ms ({lasso}), tap {:.3} ms ({tap:?})",
        t.elapsed().as_secs_f64() * 1000.0
    );
}
