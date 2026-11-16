//! What each linked Terrain3D node's clipmap holds on the GPU: a fine layer for the regions near a camera, least
//! recently used first, and a coarse atlas block for every region, uploaded within a byte budget per frame.

use std::collections::{HashMap, HashSet};

use td_formats::terrain3d_cache::LodLevel;
use td_geom::terrain3d::{Control, Loc};
use td_render::clipmap::Slot;

use super::residency::Source;

/// Bytes a frame uploads at most, though always one region, so a region of 1024 costs one frame.
pub const UPLOAD_BUDGET: usize = 24 << 20;

/// A region as the clipmap is asked to show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub loc: Loc,
    pub source: Source,
    /// Held whole on the CPU, so it can fill a fine layer and its coarse block.
    pub whole: bool,
    /// Its pyramid holds the coarse stride.
    pub pyramid: bool,
}

/// One upload of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upload {
    Coarse(Loc),
    Fine(Loc, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layer {
    loc: Loc,
    source: Source,
    used: u64,
}

/// The bookkeeping of one clipmap: which region each fine layer and coarse block holds.
#[derive(Clone, Debug, Default)]
pub struct Held {
    layers: Vec<Option<Layer>>,
    coarse: HashMap<Loc, Source>,
    tick: u64,
}

impl Held {
    pub fn new(layers: u32) -> Self {
        Self { layers: vec![None; layers as usize], ..Default::default() }
    }

    pub fn fine_layer(&self, loc: Loc) -> Option<u32> {
        self.layers.iter().position(|l| l.is_some_and(|l| l.loc == loc)).map(|i| i as u32)
    }

    pub fn fine_used(&self) -> usize {
        self.layers.iter().flatten().count()
    }

    pub fn coarse_count(&self) -> usize {
        self.coarse.len()
    }

    /// This frame's uploads, recorded as done: a coarse block for every region that has none, then fine layers for
    /// `fine` nearest first, then the coarse blocks whose source changed. Stops before the upload that would pass
    /// `budget` bytes, unless it is the first. Returns the uploads and whether some were left for the next frame.
    pub fn plan(&mut self, regions: &[Region], fine: &[Loc], bytes: [usize; 2], budget: usize) -> (Vec<Upload>, bool) {
        self.tick += 1;
        let present: HashMap<Loc, &Region> = regions.iter().map(|r| (r.loc, r)).collect();
        let near: Vec<&Region> = fine.iter().filter_map(|l| present.get(l).copied()).take(self.layers.len()).collect();
        let keep: HashSet<Loc> = near.iter().map(|r| r.loc).collect();
        self.coarse.retain(|loc, _| present.contains_key(loc));
        // Only layers of `near` are refilled, and the shader prefers a layer to the coarse block at any distance.
        for layer in &mut self.layers {
            if layer.is_some_and(|l| present.get(&l.loc).is_none_or(|r| r.source != l.source && !keep.contains(&l.loc))) {
                *layer = None;
            }
        }

        let [coarse_bytes, fine_bytes] = bytes;
        let mut out = Vec::new();
        let mut spent = 0usize;
        let mut room = |cost: usize, out: &[Upload]| {
            let fits = out.is_empty() || spent + cost <= budget;
            if fits {
                spent += cost;
            }

            fits
        };

        let missing: Vec<&Region> = regions.iter().filter(|r| !self.coarse.contains_key(&r.loc) && (r.pyramid || r.whole)).collect();
        for r in missing {
            if !room(coarse_bytes, &out) {
                return (out, true);
            }

            self.coarse.insert(r.loc, r.source);
            out.push(Upload::Coarse(r.loc));
        }

        for r in &near {
            let held = self.fine_layer(r.loc);
            if let Some(i) = held {
                let layer = self.layers[i as usize].as_mut().expect("a held layer");
                layer.used = self.tick;
                if layer.source == r.source || !r.whole {
                    continue;
                }
            }

            if !r.whole {
                continue;
            }

            let free = || self.layers.iter().position(Option::is_none);
            let stale = || {
                let unwanted = self.layers.iter().enumerate().filter(|(_, l)| l.is_some_and(|l| !keep.contains(&l.loc)));
                unwanted.min_by_key(|(_, l)| l.map(|l| l.used)).map(|(i, _)| i)
            };
            let Some(i) = held.map(|i| i as usize).or_else(free).or_else(stale) else { continue };
            if !room(fine_bytes, &out) {
                return (out, true);
            }

            self.layers[i] = Some(Layer { loc: r.loc, source: r.source, used: self.tick });
            out.push(Upload::Fine(r.loc, i as u32));
        }

        for r in regions {
            let stale = self.coarse.get(&r.loc).is_some_and(|s| *s != r.source);
            let ready = r.whole || (r.pyramid && !matches!(r.source, Source::Edited(_)));
            if stale && ready {
                if !room(coarse_bytes, &out) {
                    return (out, true);
                }

                self.coarse.insert(r.loc, r.source);
                out.push(Upload::Coarse(r.loc));
            }
        }

        (out, false)
    }

    /// What the region map should say of each region: its fine layer, else its coarse block, else absent.
    pub fn slots(&self, regions: &[Region]) -> Vec<(Loc, Slot)> {
        regions
            .iter()
            .map(|r| {
                let slot = match self.fine_layer(r.loc) {
                    Some(layer) => Slot::Fine(layer),
                    None if self.coarse.contains_key(&r.loc) => Slot::Coarse,
                    None => Slot::Absent,
                };
                (r.loc, slot)
            })
            .collect()
    }
}

/// The coarse texels of a pyramid level at the clipmap's stride, `region_size / stride` squared row by row. The splat
/// weights become a control word: the strongest texture as base, the second as overlay blended by its share of the
/// two, and the hole bit. None when the level is not at `stride`.
pub fn coarse_from_level(level: &LodLevel, region_size: u32, stride: u32) -> Option<(Vec<f32>, Vec<u32>)> {
    if level.stride != stride || stride == 0 || !region_size.is_multiple_of(stride) || level.side < region_size / stride {
        return None;
    }

    let cs = (region_size / stride) as usize;
    let side = level.side as usize;
    let mut heights = Vec::with_capacity(cs * cs);
    let mut control = Vec::with_capacity(cs * cs);
    for j in 0..cs {
        for i in 0..cs {
            let k = j * side + i;
            heights.push(level.heights[k]);
            let mut c = Control(0).with_base(level.ids.first().copied().unwrap_or(0));
            if let Some(w) = level.splat.get(k * 4..k * 4 + 4) {
                let mut order: Vec<usize> = (0..level.ids.len().min(4)).collect();
                order.sort_by_key(|&ch| std::cmp::Reverse(w[ch]));
                if let [first, rest @ ..] = order.as_slice() {
                    c = c.with_base(level.ids[*first]).with_overlay(level.ids[*first]);
                    if let Some(&second) = rest.first().filter(|&&s| w[s] > 0) {
                        let share = w[second] as f32 / (w[*first] as f32 + w[second] as f32);
                        c = c.with_overlay(level.ids[second]).with_blend((share * 255.0).round() as u8);
                    }
                }
            }

            control.push(c.with_hole(level.holes.get(k).is_some_and(|h| *h != 0)).0);
        }
    }

    Some((heights, control))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(loc: Loc, hash: u8) -> Region {
        Region { loc, source: Source::Cached([hash; 32]), whole: true, pyramid: true }
    }

    #[test]
    fn coarse_blocks_come_first_then_fine_layers_nearest_first() {
        let regions = [region([-2, 1], 1), region([3, -1], 2), region([0, 0], 3)];
        let mut held = Held::new(2);
        let (ups, more) = held.plan(&regions, &[[0, 0], [-2, 1], [3, -1]], [10, 100], usize::MAX);
        use Upload::*;
        assert_eq!(ups, vec![Coarse([-2, 1]), Coarse([3, -1]), Coarse([0, 0]), Fine([0, 0], 0), Fine([-2, 1], 1)]);
        assert!(!more);
        assert_eq!(held.slots(&regions), vec![([-2, 1], Slot::Fine(1)), ([3, -1], Slot::Coarse), ([0, 0], Slot::Fine(0))]);
        assert_eq!(held.plan(&regions, &[[0, 0], [-2, 1]], [10, 100], usize::MAX), (vec![], false), "nothing changed, nothing uploads");
        assert_eq!((held.fine_used(), held.coarse_count()), (2, 3));
    }

    #[test]
    fn with_one_region_more_than_layers_the_least_recently_used_layer_is_reused() {
        let regions: Vec<Region> = (0..4).map(|k| region([k - 1, 2 * k - 3], k as u8)).collect();
        let locs: Vec<Loc> = regions.iter().map(|r| r.loc).collect();
        let mut held = Held::new(3);
        held.plan(&regions, &locs[..3], [10, 100], usize::MAX);
        held.plan(&regions, &[locs[2], locs[0]], [10, 100], usize::MAX);
        let (ups, _) = held.plan(&regions, &[locs[3], locs[2], locs[0]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[3], 1)], "region 1 was used least recently");
        assert_eq!(held.fine_layer(locs[1]), None);
        assert_eq!(held.slots(&regions)[1], (locs[1], Slot::Coarse), "an evicted region falls back to its coarse block");

        let (ups, _) = held.plan(&regions, &[locs[1], locs[3], locs[2], locs[0]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[1], 0)], "only the three nearest wanted regions keep layers");
        let (ups, _) = held.plan(&regions, &[locs[0], locs[1], locs[3]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine(locs[0], 2)], "region 2 is no longer wanted, so its layer goes");
    }

    #[test]
    fn the_budget_stops_before_the_upload_that_passes_it_but_never_before_the_first() {
        let regions: Vec<Region> = (0..3).map(|k| region([2 * k - 1, 1 - k], k as u8)).collect();
        let locs: Vec<Loc> = regions.iter().map(|r| r.loc).collect();
        let mut held = Held::new(3);
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 5);
        assert_eq!((ups, more), (vec![Upload::Coarse(locs[0])], true), "one upload even over the budget");
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 120);
        assert_eq!((ups, more), (vec![Upload::Coarse(locs[1]), Upload::Coarse(locs[2]), Upload::Fine(locs[0], 0)], true));
        let (ups, more) = held.plan(&regions, &locs, [10, 100], 200);
        assert_eq!((ups, more), (vec![Upload::Fine(locs[1], 1), Upload::Fine(locs[2], 2)], false), "exactly the budget fits");
    }

    #[test]
    fn a_changed_source_reuploads_its_layer_and_block_and_a_removed_region_frees_both() {
        let mut regions = vec![region([5, -4], 1), region([-6, 7], 2)];
        let mut held = Held::new(2);
        held.plan(&regions, &[[5, -4], [-6, 7]], [10, 100], usize::MAX);
        regions[0].source = Source::Edited(0x40);
        regions[1].whole = false;
        regions[1].source = Source::Cached([9; 32]);
        let (ups, _) = held.plan(&regions, &[[5, -4], [-6, 7]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine([5, -4], 0), Upload::Coarse([5, -4]), Upload::Coarse([-6, 7])]);
        assert_eq!(held.slots(&regions)[1], ([-6, 7], Slot::Fine(1)), "a stale layer still shows until the region loads whole");

        let gone = [regions[1]];
        assert_eq!(held.plan(&gone, &[[5, -4], [-6, 7]], [10, 100], usize::MAX).0, vec![]);
        assert_eq!((held.fine_layer([5, -4]), held.fine_used(), held.coarse_count()), (None, 1, 1));
        assert_eq!(held.slots(&[]), vec![]);
    }

    #[test]
    fn a_changed_region_that_gets_no_new_layer_this_frame_falls_back_to_its_coarse_block() {
        let mut regions = vec![Region { source: Source::Edited(0x50), ..region([7, -2], 1) }, region([-3, 4], 2), region([1, 6], 3)];
        let mut held = Held::new(2);
        held.plan(&regions, &[[7, -2], [-3, 4]], [10, 100], usize::MAX);
        regions[0].source = Source::Cached([1; 32]);
        assert_eq!(held.plan(&regions, &[], [10, 100], usize::MAX), (vec![Upload::Coarse([7, -2])], false), "an undone edit far away");
        assert_eq!(held.slots(&regions), vec![([7, -2], Slot::Coarse), ([-3, 4], Slot::Fine(1)), ([1, 6], Slot::Coarse)], "an unchanged layer stays");

        regions[1].source = Source::Edited(0x60);
        regions[2].whole = false;
        let (ups, _) = held.plan(&regions, &[[7, -2], [1, 6], [-3, 4]], [10, 100], usize::MAX);
        assert_eq!(ups, vec![Upload::Fine([7, -2], 0), Upload::Coarse([-3, 4])]);
        assert_eq!(held.slots(&regions)[1], ([-3, 4], Slot::Coarse), "a changed region past the layer count loses its layer too");
    }

    #[test]
    fn a_region_with_neither_whole_data_nor_a_pyramid_stays_absent() {
        let r = Region { loc: [1, 1], source: Source::Cached([1; 32]), whole: false, pyramid: false };
        let mut held = Held::new(1);
        assert_eq!(held.plan(&[r], &[[1, 1]], [10, 100], usize::MAX), (vec![], false));
        assert_eq!(held.slots(&[r]), vec![([1, 1], Slot::Absent)]);
        let edited = Region { source: Source::Edited(8), pyramid: true, ..r };
        held.plan(&[Region { pyramid: true, ..r }], &[], [10, 100], usize::MAX);
        assert_eq!(held.plan(&[edited], &[], [10, 100], usize::MAX).0, vec![], "an edit not held whole keeps the block it had");
    }

    fn level(stride: u32, side: u32) -> LodLevel {
        let n = (side * side) as usize;
        LodLevel { stride, side, ids: vec![3, 9, 17], heights: (0..n).map(|k| k as f32 * 0.5 - 7.0).collect(), splat: vec![0; n * 4], holes: vec![0; n] }
    }

    #[test]
    fn splat_weights_become_base_overlay_and_blend_and_holes_the_hole_bit() {
        let mut l = level(4, 17);
        l.splat[..4].copy_from_slice(&[60, 200, 0, 0]);
        l.splat[4..8].copy_from_slice(&[0, 0, 255, 0]);
        l.splat[(17 + 2) * 4..(17 + 2) * 4 + 4].copy_from_slice(&[90, 90, 30, 0]);
        l.holes[17 * 3 + 5] = 1;
        let (heights, control) = coarse_from_level(&l, 64, 4).unwrap();
        assert_eq!((heights.len(), control.len()), (256, 256));
        assert_eq!(heights[16 * 3 + 5], l.heights[17 * 3 + 5], "the seam column is skipped");

        let c = |k: usize| Control(control[k]);
        assert_eq!((c(0).base(), c(0).overlay(), c(0).blend(), c(0).hole()), (9, 3, 59, false), "200 beats 60, blend 60 / 260");
        assert_eq!((c(1).base(), c(1).overlay(), c(1).blend()), (17, 17, 0), "one texture alone");
        assert_eq!((c(18).base(), c(18).overlay(), c(18).blend()), (3, 9, 128), "a tie goes to the first channel");
        assert!(c(16 * 3 + 5).hole() && control.iter().filter(|w| Control(**w).hole()).count() == 1);
        assert_eq!((c(2).base(), c(2).overlay(), c(2).blend()), (3, 3, 0), "no weight at all draws the first texture");
    }

    #[test]
    fn a_level_without_splat_or_at_another_stride_converts_plainly_or_not_at_all() {
        let mut l = level(16, 64);
        l.splat.clear();
        l.holes.clear();
        let (heights, control) = coarse_from_level(&l, 1024, 16).unwrap();
        assert_eq!(heights, l.heights);
        assert!(control.iter().all(|w| *w == Control(0).with_base(3).0));
        assert_eq!(coarse_from_level(&l, 1024, 4), None);
        assert_eq!(coarse_from_level(&level(16, 63), 1024, 16), None, "too few vertices");
    }
}
