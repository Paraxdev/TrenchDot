//! Placing things along a [`Curve`]: rigid copies, a piece bent to follow it, or a cable hanging between points.

use std::f64::consts::TAU;

use td_core::{Aabb, DMat4, DVec2, DVec3, DVec4, Plane};

use crate::brush::FaceData;
use crate::mesh::{Mesh, MeshFace};
use crate::sweep::Curve;
use crate::uv::FaceUv;

/// Most copies one placement makes, so a tiny spacing on a long curve cannot stall the editor.
pub const MAX_COPIES: usize = 1024;

/// A bent piece is cut across about this often, in map units, so it can follow the curve.
const BEND_STEP: f64 = 16.0;

/// Most faces one bend may make, so a detailed piece on a long curve cannot stall the editor.
pub const MAX_BENT_FACES: usize = 100_000;

/// How a piece lies before it is placed. It runs `length` along `along` from `anchor`, the middle of its lowest side
/// at its start, with `side` across it and world up as its up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub anchor: DVec3,
    pub along: DVec3,
    pub side: DVec3,
    pub length: f64,
}

impl Piece {
    /// Runs along the longer of the bounds' X and Z sides.
    pub fn of(b: &Aabb) -> Piece {
        let size = b.max - b.min;
        let along = if size.z > size.x { DVec3::Z } else { DVec3::X };
        let length = along.dot(size);
        let center = b.center();
        let anchor = DVec3::new(center.x, b.min.y, center.z) - along * (length / 2.0);
        Piece { anchor, along, side: along.cross(DVec3::Y), length }
    }

    /// Distance along, height and offset to the side of `p`, from the anchor.
    fn local(&self, p: DVec3) -> DVec3 {
        let d = p - self.anchor;
        DVec3::new(d.dot(self.along), d.y, d.dot(self.side))
    }

    /// Maps world points onto the piece's own axes, from the anchor moved `at` along it.
    fn to_local(self, at: f64) -> DMat4 {
        let axes = DMat4::from_cols(self.along.extend(0.0), DVec3::Y.extend(0.0), self.side.extend(0.0), DVec4::W);
        axes.transpose() * DMat4::from_translation(-(self.anchor + self.along * at))
    }
}

/// Up and side for a tangent, with up kept as close to world up as the tangent allows.
fn basis(tangent: DVec3) -> (DVec3, DVec3) {
    let side = tangent.cross(DVec3::Y).try_normalize().unwrap_or_else(|| tangent.cross(DVec3::X).normalize());
    (side.cross(tangent).normalize(), side)
}

/// Position, tangent, up and side at `t` along the curve. `upright` levels the tangent, so up stays world up.
fn frame(curve: &Curve, t: f64, upright: bool) -> (DVec3, DVec3, DVec3, DVec3) {
    let (pos, mut tangent) = curve.point(t);
    if upright {
        tangent = DVec3::new(tangent.x, 0.0, tangent.z).try_normalize().unwrap_or(DVec3::X);
    }

    let (up, side) = basis(tangent);
    (pos, tangent, up, side)
}

/// Places for copies of the piece about every `spacing` units along the curve, evened out so the first sits on its
/// start and the last on its end, each turned to follow it. `upright` keeps them standing straight on a slope, like
/// fence posts.
pub fn repeat(curve: &Curve, piece: &Piece, spacing: f64, upright: bool) -> Vec<DMat4> {
    let len = curve.length();
    let gaps = ((len / spacing.max(1.0)).round() as usize).clamp(1, MAX_COPIES - 1);
    let to_local = piece.to_local(piece.length / 2.0);
    (0..=gaps)
        .map(|k| {
            let (pos, tangent, up, side) = frame(curve, k as f64 / gaps as f64, upright);
            DMat4::from_cols(tangent.extend(0.0), up.extend(0.0), side.extend(0.0), pos.extend(1.0)) * to_local
        })
        .collect()
}

/// How many copies of the piece [`bend`] lays end to end.
pub fn bend_copies(curve: &Curve, piece: &Piece) -> usize {
    ((curve.length() / piece.length.max(1e-6)).round() as usize).clamp(1, MAX_COPIES)
}

/// How many lengths [`bend`] cuts each copy of a piece with `faces` faces into, fewer when the copies would pass
/// [`MAX_BENT_FACES`], or None when even uncut copies would.
pub fn bend_cuts(curve: &Curve, piece: &Piece, faces: usize) -> Option<usize> {
    let copies = bend_copies(curve, piece);
    let wanted = ((curve.length() / copies as f64 / BEND_STEP).ceil() as usize).clamp(1, 64);
    let room = MAX_BENT_FACES / (copies * faces.max(1));
    (room > 0).then(|| wanted.min(room))
}

/// `mesh` laid end to end along the curve as often as it fits whole and bent to follow it, stretched a little to
/// close the gap. Each copy is cut across into `cuts` lengths first so it can bend, and its faces keep the texture
/// they had laid out straight, written as explicit UVs from the size `tex_size` gives their material.
pub fn bend(mesh: &Mesh, piece: &Piece, curve: &Curve, cuts: usize, tex_size: &dyn Fn(&str) -> DVec2) -> Option<Mesh> {
    if piece.length < 1e-6 || mesh.faces.is_empty() {
        return None;
    }

    let len = curve.length();
    let copies = bend_copies(curve, piece);
    let stretch = len / (copies as f64 * piece.length);
    let mut out = Mesh { smooth_angle: mesh.smooth_angle, ..Default::default() };
    for k in 0..copies {
        let offset = k as f64 * piece.length;
        let mut copy = mesh.translated(piece.along * offset, false);
        for i in 1..cuts {
            let at = piece.anchor + piece.along * (offset + piece.length * i as f64 / cuts as f64);
            copy.bisect(&Plane::from_point_normal(at, piece.along), None);
        }

        for fi in 0..copy.faces.len() {
            let size = tex_size(&copy.faces[fi].data.material);
            copy.bake_uvs(&[fi], size);
        }

        for v in &mut copy.vertices {
            let l = piece.local(*v);
            let (pos, _, up, side) = frame(curve, (l.x * stretch / len).clamp(0.0, 1.0), false);
            *v = pos + up * l.y + side * l.z;
        }

        out.join(&copy);
    }

    Some(out)
}

/// A round cable through `points`, hanging `sag` units low in the middle of each span between them. Each span is cut
/// into `steps` lengths and the cable has `sides` sides, with its ends closed.
pub fn cable(points: &[DVec3], sag: f64, radius: f64, sides: u32, steps: u32, data: &FaceData) -> Option<Mesh> {
    let mut pts: Vec<DVec3> = Vec::new();
    for p in points {
        if pts.last().is_none_or(|l| p.distance(*l) > 1e-6) {
            pts.push(*p);
        }
    }

    if pts.len() < 2 || radius <= 0.0 {
        return None;
    }

    let (sides, steps) = (sides.clamp(3, 64) as usize, steps.clamp(1, 64) as usize);
    let mut path = vec![pts[0]];
    for w in pts.windows(2) {
        for k in 1..=steps {
            let u = k as f64 / steps as f64;
            path.push(w[0].lerp(w[1], u) - DVec3::Y * sag * 4.0 * u * (1.0 - u));
        }
    }

    let last = path.len() - 1;
    let mut mesh = Mesh { smooth_angle: 80.0, ..Default::default() };
    for i in 0..=last {
        let tangent = (path[(i + 1).min(last)] - path[i.saturating_sub(1)]).normalize();
        let (up, side) = basis(tangent);
        for j in 0..sides {
            let a = TAU * j as f64 / sides as f64;
            mesh.vertices.push(path[i] + (up * a.cos() + side * a.sin()) * radius);
        }
    }

    let ring = |i: usize, j: usize| (i * sides + j % sides) as u32;
    let add = |mesh: &mut Mesh, indices: Vec<u32>| {
        let pts: Vec<DVec3> = indices.iter().map(|i| mesh.vertices[*i as usize]).collect();
        let normal = (pts[1] - pts[0]).cross(pts[2] - pts[0]).normalize_or(DVec3::Y);
        let mut face_data = data.clone();
        face_data.uv = FaceUv::face_aligned(normal, DVec2::ONE);
        mesh.faces.push(MeshFace::new(indices, face_data));
    };
    for i in 0..last {
        for j in 0..sides {
            add(&mut mesh, vec![ring(i, j), ring(i, j + 1), ring(i + 1, j + 1), ring(i + 1, j)]);
        }
    }

    add(&mut mesh, (0..sides).rev().map(|j| ring(0, j)).collect());
    add(&mut mesh, (0..sides).map(|j| ring(last, j)).collect());
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: DVec3, b: DVec3) -> bool {
        a.distance(b) < 0.5
    }

    fn post() -> (Piece, Aabb) {
        let b = Aabb::new(DVec3::new(-8.0, 0.0, -8.0), DVec3::new(8.0, 64.0, 8.0));
        (Piece::of(&b), b)
    }

    #[test]
    fn copies_spread_evenly_from_the_start_to_the_end() {
        let curve = Curve::through(&[DVec3::ZERO, DVec3::new(250.0, 0.0, 0.0)]).unwrap();
        let (piece, b) = post();
        let places = repeat(&curve, &piece, 64.0, true);
        assert_eq!(places.len(), 5);
        let foot = DVec3::new(b.center().x, b.min.y, b.center().z);
        assert!(close(places[0].transform_point3(foot), DVec3::ZERO));
        assert!(close(places[4].transform_point3(foot), DVec3::new(250.0, 0.0, 0.0)));
        assert!(close(places[2].transform_point3(foot), DVec3::new(125.0, 0.0, 0.0)));
    }

    #[test]
    fn upright_copies_stand_straight_on_a_slope_and_others_lean_with_it() {
        let curve = Curve::through(&[DVec3::ZERO, DVec3::new(256.0, 128.0, 0.0)]).unwrap();
        let (piece, _) = post();
        let up = |m: &DMat4| m.transform_vector3(DVec3::Y).normalize();
        assert!(repeat(&curve, &piece, 64.0, true).iter().all(|m| close(up(m) * 100.0, DVec3::Y * 100.0)));
        assert!(repeat(&curve, &piece, 64.0, false).iter().all(|m| up(m).x < -0.3));
    }

    #[test]
    fn a_turning_curve_turns_the_copies_with_it() {
        let curve = Curve::through(&[DVec3::ZERO, DVec3::new(200.0, 0.0, 0.0), DVec3::new(200.0, 0.0, 200.0)]).unwrap();
        let b = Aabb::new(DVec3::new(0.0, 0.0, -4.0), DVec3::new(32.0, 16.0, 4.0));
        let places = repeat(&curve, &Piece::of(&b), 64.0, true);
        let last = places.last().unwrap();
        assert!(close(last.transform_vector3(DVec3::X) * 10.0, DVec3::Z * 10.0), "{:?}", last.transform_vector3(DVec3::X));
    }

    #[test]
    fn a_piece_long_on_z_runs_along_the_curve_without_mirroring() {
        let rail = Aabb::new(DVec3::new(-2.0, 0.0, -32.0), DVec3::new(2.0, 8.0, 32.0));
        let piece = Piece::of(&rail);
        assert_eq!(piece.along, DVec3::Z);
        let curve = Curve::through(&[DVec3::ZERO, DVec3::new(256.0, 0.0, 0.0)]).unwrap();
        let places = repeat(&curve, &piece, 64.0, true);
        assert_eq!(places.len(), 5);
        let (start, end) = (DVec3::new(0.0, 4.0, -32.0), DVec3::new(0.0, 4.0, 32.0));
        assert!(close(places[0].transform_point3(start), DVec3::new(-32.0, 4.0, 0.0)), "{}", places[0].transform_point3(start));
        assert!(places.windows(2).all(|w| close(w[0].transform_point3(end), w[1].transform_point3(start))), "rails meet end to end");
        assert!(places.iter().all(|m| m.determinant() > 0.0), "copies are turned, never mirrored");

        let slab = Mesh::from_brush(&crate::brush::Brush::from_aabb(&Aabb::new(DVec3::new(-16.0, 0.0, 0.0), DVec3::new(16.0, 8.0, 64.0)), "dev/grey").unwrap());
        let piece = Piece::of(&slab.bounds());
        assert_eq!(piece.along, DVec3::Z);
        let straight = Curve::through(&[DVec3::new(0.0, 0.0, 100.0), DVec3::new(250.0, 0.0, 100.0)]).unwrap();
        let cuts = bend_cuts(&straight, &piece, slab.faces.len()).unwrap();
        let out = bend(&slab, &piece, &straight, cuts, &|_| DVec2::splat(64.0)).unwrap();
        let b = out.bounds();
        assert!(close(b.min, DVec3::new(0.0, 0.0, 84.0)) && close(b.max, DVec3::new(250.0, 8.0, 116.0)), "{b:?}");
        assert!((out.volume() - 250.0 * 8.0 * 32.0).abs() < 1.0, "faces still point out, {}", out.volume());
    }

    #[test]
    fn a_heavy_piece_is_cut_less_and_one_too_heavy_to_bend_is_refused() {
        let piece = Piece::of(&Aabb::new(DVec3::ZERO, DVec3::new(64.0, 8.0, 8.0)));
        let short = Curve::through(&[DVec3::ZERO, DVec3::new(256.0, 0.0, 0.0)]).unwrap();
        assert_eq!(bend_cuts(&short, &piece, 6), Some(4));
        let long = Curve::through(&[DVec3::ZERO, DVec3::new(65536.0, 0.0, 0.0)]).unwrap();
        assert_eq!(bend_copies(&long, &piece), MAX_COPIES);
        assert_eq!(bend_cuts(&long, &piece, 6), Some(4));
        assert_eq!(bend_cuts(&long, &piece, 40), Some(MAX_BENT_FACES / (MAX_COPIES * 40)));
        assert_eq!(bend_cuts(&long, &piece, MAX_BENT_FACES / MAX_COPIES + 1), None);
    }

    #[test]
    fn a_bent_piece_fills_the_curve_with_whole_stretched_copies() {
        let slab = Mesh::from_brush(&crate::brush::Brush::from_aabb(&Aabb::new(DVec3::new(0.0, 0.0, -32.0), DVec3::new(64.0, 8.0, 32.0)), "dev/grey").unwrap());
        let piece = Piece::of(&slab.bounds());
        assert_eq!(piece.along, DVec3::X);
        let straight = Curve::through(&[DVec3::new(0.0, 0.0, 100.0), DVec3::new(250.0, 0.0, 100.0)]).unwrap();
        assert_eq!(bend_copies(&straight, &piece), 4);
        let cuts = |curve: &Curve| bend_cuts(curve, &piece, slab.faces.len()).unwrap();
        let out = bend(&slab, &piece, &straight, cuts(&straight), &|_| DVec2::splat(64.0)).unwrap();
        let b = out.bounds();
        assert!(close(b.min, DVec3::new(0.0, 0.0, 68.0)) && close(b.max, DVec3::new(250.0, 8.0, 132.0)), "{b:?}");
        assert!(out.faces.iter().all(|f| f.uvs.len() == f.indices.len()));

        let turn = Curve::through(&[DVec3::ZERO, DVec3::new(300.0, 0.0, 0.0), DVec3::new(300.0, 0.0, 300.0)]).unwrap();
        let out = bend(&slab, &piece, &turn, cuts(&turn), &|_| DVec2::splat(64.0)).unwrap();
        let end = out.vertices.iter().filter(|v| v.y < 1.0).map(|v| v.distance(DVec3::new(300.0, 0.0, 300.0))).fold(f64::MAX, f64::min);
        assert!(end <= 32.5, "the last copy reaches the last point, {end}");
        assert!(out.vertices.len() > slab.vertices.len() * bend_copies(&turn, &piece), "copies are cut so they can bend");
    }

    #[test]
    fn a_cable_hangs_between_its_points_and_is_closed() {
        let data = FaceData::new("dev/dark", FaceUv::default());
        let mesh = cable(&[DVec3::new(0.0, 100.0, 0.0), DVec3::new(200.0, 100.0, 0.0)], 30.0, 2.0, 6, 8, &data).unwrap();
        assert!(mesh.is_closed() && mesh.validate().is_ok());
        let low = mesh.vertices.iter().map(|v| v.y).fold(f64::MAX, f64::min);
        assert!((low - 68.0).abs() < 0.5, "{low}");
        assert!(cable(&[DVec3::ZERO], 30.0, 2.0, 6, 8, &data).is_none());
    }
}
