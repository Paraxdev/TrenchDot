//! Keeps a native terrain's clipmap textures in step with the map. The GPU holds the previous map's terrain, so an edit
//! writes only the rectangle of vertices that differs, like `changed_chunks` rebuilds only the chunks that differ.

use glam::Vec3;
use td_geom::Terrain;
use td_render::Renderer;
use td_render::clipmap::native::{TerrainClipParams, TerrainClipmap};

/// Vertices whose texels differ between two versions of a terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texels {
    Whole,
    Rect { origin: [u32; 2], size: [u32; 2] },
}

/// How a native terrain draws while level of detail is on.
pub enum TerrainClip {
    Clipmap(Box<TerrainClipmap>),
    /// Drawn with chunks because the GPU could not make a clipmap of this resolution.
    Fallback {
        resolution: [u32; 2],
        reason: String,
    },
}

impl TerrainClip {
    pub fn clipmap(&self) -> Option<&TerrainClipmap> {
        match self {
            TerrainClip::Clipmap(clip) => Some(&**clip),
            TerrainClip::Fallback { .. } => None,
        }
    }

    pub fn fallback(&self) -> Option<&str> {
        match self {
            TerrainClip::Clipmap(_) => None,
            TerrainClip::Fallback { reason, .. } => Some(reason),
        }
    }
}

/// The paint texel of vertex (i, j): its four splat bytes, layer 0 in the low byte and 0 without splat, and in bit 0
/// of y whether the cell starting there is a hole.
fn texel(t: &Terrain, i: u32, j: u32) -> [u32; 2] {
    let k = t.index(i, j) * 4;
    let splat = t.splat.get(k..k + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map_or(0, u32::from_le_bytes);
    let [cx, cz] = t.cells();
    [splat, (i < cx && j < cz && t.is_hole(i, j)) as u32]
}

fn holes_row(t: &Terrain, j: u32) -> Option<&[u8]> {
    let cells = t.cells()[0] as usize;
    t.holes.get(j as usize * cells..(j as usize + 1) * cells)
}

/// The smallest rectangle holding every vertex whose height or paint texel differs, None when none does. Origin, cell
/// size and layers live in the uniform and the material, so they never make texels differ.
pub fn changed_rect(old: &Terrain, new: &Terrain) -> Option<Texels> {
    if old.resolution != new.resolution || !old.is_valid() || !new.is_valid() {
        return Some(Texels::Whole);
    }

    let [w, d] = new.resolution;
    let same = |i: u32, j: u32| {
        let k = new.index(i, j);
        old.heights[k].to_bits() == new.heights[k].to_bits() && texel(old, i, j) == texel(new, i, j)
    };
    let mut rect: Option<([u32; 2], [u32; 2])> = None;
    for j in 0..d {
        let (a, b) = (new.index(0, j), new.index(0, j) + w as usize);
        let rows_match = old.heights[a..b].iter().zip(&new.heights[a..b]).all(|(x, y)| x.to_bits() == y.to_bits())
            && old.splat.get(a * 4..b * 4) == new.splat.get(a * 4..b * 4)
            && holes_row(old, j) == holes_row(new, j);
        if rows_match {
            continue;
        }

        let Some(first) = (0..w).find(|&i| !same(i, j)) else {
            continue;
        };
        let last = (first..w).rev().find(|&i| !same(i, j)).unwrap_or(first);
        rect = Some(match rect {
            None => ([first, j], [last, j]),
            Some((lo, hi)) => ([lo[0].min(first), lo[1]], [hi[0].max(last), j]),
        });
    }

    rect.map(|(lo, hi)| Texels::Rect { origin: lo, size: [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1] })
}

/// Heights and paint texels of a rectangle of vertices, row by row, the way [`TerrainClipmap::write_rect`] takes them.
pub fn pack(t: &Terrain, origin: [u32; 2], size: [u32; 2]) -> (Vec<f32>, Vec<[u32; 2]>) {
    let n = size[0] as usize * size[1] as usize;
    let (mut heights, mut paint) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for j in origin[1]..origin[1] + size[1] {
        for i in origin[0]..origin[0] + size[0] {
            heights.push(t.heights.get(t.index(i, j)).copied().unwrap_or(0.0));
            paint.push(texel(t, i, j));
        }
    }

    (heights, paint)
}

pub fn params(t: &Terrain, selected: bool, chart: Option<[[f32; 4]; 2]>) -> TerrainClipParams {
    TerrainClipParams { origin: Vec3::new(t.origin.x as f32, t.origin.y as f32, t.origin.z as f32), cell: t.cell_size as f32, selected, chart }
}

fn write(renderer: &Renderer, clip: &mut TerrainClipmap, t: &Terrain, texels: Texels) -> Result<(), String> {
    match texels {
        Texels::Whole => {
            let (heights, paint) = pack(t, [0, 0], t.resolution);
            clip.write_whole(&renderer.queue, &heights, &paint)
        }
        Texels::Rect { origin, size } => {
            let (heights, paint) = pack(t, origin, size);
            clip.write_rect(&renderer.queue, origin, size, &heights, &paint)
        }
    }
}

/// Brings a terrain's clipmap up to `new`. `previous` holds `old` on the GPU, so only the texels that differ are
/// written, while a new clipmap or one without `old` gets every texel. A fallback stays one until the resolution
/// changes, so a terrain too big for the GPU is not tried again on every edit.
pub fn sync(
    renderer: &Renderer,
    previous: Option<TerrainClip>,
    old: Option<&Terrain>,
    new: &Terrain,
    params: &TerrainClipParams,
    material: &str,
) -> TerrainClip {
    let fallback = |reason: String| TerrainClip::Fallback { resolution: new.resolution, reason };
    let (mut clip, texels) = match previous {
        Some(TerrainClip::Fallback { resolution, reason }) if resolution == new.resolution => return TerrainClip::Fallback { resolution, reason },
        Some(TerrainClip::Clipmap(clip)) if clip.resolution() == new.resolution => (clip, old.map_or(Some(Texels::Whole), |old| changed_rect(old, new))),
        _ => match renderer.new_terrain_clipmap(new.resolution) {
            Ok(clip) => (Box::new(clip), Some(Texels::Whole)),
            Err(reason) => return fallback(reason),
        },
    };
    if let Some(texels) = texels
        && let Err(reason) = write(renderer, &mut clip, new, texels)
    {
        return fallback(reason);
    }

    if clip.params() != params {
        clip.set_params(&renderer.queue, params);
    }

    if clip.material() != material {
        clip.set_material(material);
    }

    TerrainClip::Clipmap(clip)
}

#[cfg(test)]
mod tests {
    use td_core::DVec3;
    use td_geom::TerrainLayer;

    use super::*;

    /// Off the origin, not square, a cell size other than 32, with splat everywhere and a hole in the last cell.
    fn terrain() -> Terrain {
        let mut t = Terrain::new(DVec3::new(-1000.5, 37.0, 2210.0), [37, 21], 24.0, "dirt");
        for (k, h) in t.heights.iter_mut().enumerate() {
            *h = (k as f32 * 0.37).sin() * 90.0;
        }

        t.splat = (0..t.heights.len() * 4).map(|k| (k * 29 % 256) as u8).collect();
        let [cx, cz] = t.cells();
        t.holes = vec![0; (cx * cz) as usize];
        *t.holes.last_mut().unwrap() = 1;
        t
    }

    fn one(origin: [u32; 2]) -> Option<Texels> {
        Some(Texels::Rect { origin, size: [1, 1] })
    }

    #[test]
    fn identical_data_and_uniform_only_changes_write_no_texels() {
        let t = terrain();
        assert_eq!(changed_rect(&t, &t.clone()), None);

        let mut moved = t.clone();
        moved.origin = DVec3::new(64.0, -12.0, 8.0);
        moved.cell_size = 48.0;
        moved.layers.push(TerrainLayer::new("rock", 512.0));
        moved.chunk_cells = 8;
        assert_eq!(changed_rect(&t, &moved), None);

        let mut bare = Terrain::new(t.origin, t.resolution, t.cell_size, "dirt");
        let mut zeros = bare.clone();
        zeros.splat = vec![0; bare.heights.len() * 4];
        zeros.holes = vec![0; (bare.cells()[0] * bare.cells()[1]) as usize];
        assert_eq!(changed_rect(&bare, &zeros), None, "no splat packs as zero bytes and no holes as open cells");
        bare.heights[0] = 1.0;
        assert_eq!(changed_rect(&zeros, &bare), one([0, 0]));
    }

    #[test]
    fn one_changed_height_splat_byte_or_hole_writes_exactly_its_vertex() {
        let t = terrain();
        let mut raised = t.clone();
        raised.heights[t.index(5, 7)] += 0.25;
        assert_eq!(changed_rect(&t, &raised), one([5, 7]));

        let mut painted = t.clone();
        painted.splat[t.index(36, 20) * 4 + 3] ^= 1;
        assert_eq!(changed_rect(&t, &painted), one([36, 20]));

        let mut filled = t.clone();
        *filled.holes.last_mut().unwrap() = 0;
        assert_eq!(changed_rect(&t, &filled), one([35, 19]), "the last cell's hole bit sits on its first vertex");

        let mut both = raised.clone();
        both.splat[t.index(30, 2) * 4] ^= 8;
        assert_eq!(changed_rect(&t, &both), Some(Texels::Rect { origin: [5, 2], size: [26, 6] }));
    }

    #[test]
    fn a_resolution_change_writes_every_texel() {
        let t = terrain();
        let wider = Terrain::new(t.origin, [38, 21], t.cell_size, "dirt");
        assert_eq!(changed_rect(&t, &wider), Some(Texels::Whole));
    }

    #[test]
    fn a_packed_rect_matches_the_same_texels_of_a_whole_pack() {
        let t = terrain();
        let [w, d] = t.resolution;
        let (heights, paint) = pack(&t, [0, 0], t.resolution);
        assert_eq!(heights, t.heights);
        assert_eq!(paint.len(), (w * d) as usize);

        let (origin, size) = ([30, 14], [7, 7]);
        let (rect_heights, rect_paint) = pack(&t, origin, size);
        for (n, (h, p)) in rect_heights.iter().zip(&rect_paint).enumerate() {
            let (i, j) = (origin[0] + n as u32 % size[0], origin[1] + n as u32 / size[0]);
            let k = t.index(i, j);
            assert_eq!((*h, *p), (heights[k], paint[k]), "vertex ({i}, {j})");
        }

        let k = t.index(4, 3);
        assert_eq!(paint[k][0].to_le_bytes(), t.splat[k * 4..k * 4 + 4], "layer 0 is the low byte");
        assert_eq!(paint[t.index(35, 19)][1], 1);
        assert_eq!(paint.iter().filter(|p| p[1] != 0).count(), 1);
        assert!((0..d).all(|j| paint[t.index(w - 1, j)][1] == 0) && (0..w).all(|i| paint[t.index(i, d - 1)][1] == 0));
    }
}
