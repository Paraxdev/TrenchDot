//! Mirrored linked copies: members of a link set placed as reflections of each other across an axis plane, so an edit
//! in one shows mirrored in the other.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use td_core::{Aabb, AxisPlane, DMat4, DVec3, NodeId};
use td_geom::{Brush, csg};

use crate::linked;
use crate::map::{Map, NodeKind};
use crate::ops::{self, EditOptions, Replaced};
use crate::selection::Selection;
use crate::symmetry::{MIRROR_TOLERANCE, PlaneChoice};

/// A plane between a linked group and a member of its set that mirrors it, as the views draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorPlane {
    pub plane: AxisPlane,
    pub group: NodeId,
    pub partner: NodeId,
    /// Around both halves.
    pub span: Aabb,
}

fn link_of(map: &Map, id: NodeId) -> Option<u64> {
    match map.get(id).map(|n| &n.kind) {
        Some(NodeKind::Group(g)) => g.link_id,
        _ => None,
    }
}

/// Gives each selected group a linked copy reflected across the plane `choice` puts on `axis` for the groups' bounds,
/// and selects the copies. A `gap` moves the plane half of it away from the groups, so they stay where they are and
/// the copies land `gap` further off. Returns None and changes nothing without a selected group or a plane.
pub fn mirror_linked(map: &mut Map, sel: &mut Selection, axis: usize, choice: &PlaneChoice, gap: f64) -> Option<(Vec<NodeId>, AxisPlane)> {
    let is_group = |id: &NodeId| matches!(map.get(*id).map(|n| &n.kind), Some(NodeKind::Group(_)));
    let groups: Vec<NodeId> = ops::selection_roots(map, sel).into_iter().filter(is_group).collect();
    if groups.is_empty() {
        return None;
    }

    let bounds = map.bounds_of(groups.iter().copied());
    let plane = choice.resolve(axis, &bounds)?;
    let mut solid = Aabb::EMPTY;
    for g in &groups {
        solid.include(&map.solid_bounds(*g));
    }

    let plane = widened(plane, &bounds, &solid, gap.min(MAX_GAP));
    // Sync maps edits between copies with UV lock on, a copy made without it would change its textures on the first edit.
    let opts = EditOptions { uv_lock: true, ..EditOptions::default() };
    let copies = ops::duplicate_linked(map, sel, DVec3::ZERO, opts);
    ops::transform_selection(map, sel, &plane.matrix(), opts);
    Some((copies, plane))
}

/// How far short of covering each other the halves stop when a gap pulls them in. Once they cover each other exactly
/// nothing tells which one was lower, and the next gap could swap them.
const COVER_SLACK: f64 = 1.0 / 4096.0;

/// The widest gap [`mirror_linked`] and [`set_gap`] put between halves, so a typo cannot throw a half out of the world.
pub const MAX_GAP: f64 = 65536.0;

/// `plane` moved `gap` / 2 away from the original within `bounds`. It stops [`COVER_SLACK`] / 2 short of the centre of
/// the `solid` bounds, where the copy covers the original, the same limit as [`set_gap`].
fn widened(plane: AxisPlane, bounds: &Aabb, solid: &Aabb, gap: f64) -> AxisPlane {
    let (p, axis) = (plane.at, plane.axis);
    let (centre, slack) = (solid.center()[axis], COVER_SLACK / 2.0);
    let at = if p >= bounds.center()[axis] { (p + gap / 2.0).max(p.min(centre + slack)) } else { (p - gap / 2.0).min(p.max(centre - slack)) };
    AxisPlane { axis, at }
}

/// The other members of `group`'s link set placed as its mirror image, each with the plane between them, by id.
pub fn partners(map: &Map, group: NodeId) -> Vec<(NodeId, AxisPlane)> {
    let Some(NodeKind::Group(g)) = map.get(group).map(|n| &n.kind) else { return Vec::new() };
    let Some(link) = g.link_id else { return Vec::new() };
    let members = map.nodes.iter().filter_map(|(id, node)| match &node.kind {
        NodeKind::Group(other) if other.link_id == Some(link) => Some((*id, other)),
        _ => None,
    });
    mirrors(group, g, members)
}

/// The `members` placed as the mirror image of `group`, which is `g`.
fn mirrors<'a>(group: NodeId, g: &crate::Group, members: impl Iterator<Item = (NodeId, &'a crate::Group)>) -> Vec<(NodeId, AxisPlane)> {
    let (inverse, flipped) = (g.transform.inverse(), g.transform.determinant() < 0.0);
    members
        .filter(|(id, other)| *id != group && (other.transform.determinant() < 0.0) != flipped)
        .filter_map(|(id, other)| AxisPlane::of_reflection(&(other.transform * inverse)).map(|p| (id, p)))
        .collect()
}

/// The members of every link set that holds a mirror image, gathered in one walk of the map. A set of plain linked
/// copies, where every transform keeps its handedness, holds none and is left out.
struct MirrorSets<'a>(HashMap<u64, Vec<(NodeId, &'a crate::Group)>>);

impl<'a> MirrorSets<'a> {
    fn of(map: &'a Map) -> Self {
        let mut sets: HashMap<u64, Vec<(NodeId, &crate::Group)>> = HashMap::new();
        for (id, node) in map.nodes.iter() {
            if let NodeKind::Group(g) = &node.kind
                && let Some(link) = g.link_id
            {
                sets.entry(link).or_default().push((*id, g));
            }
        }

        sets.retain(|_, members| {
            let flipped = members.iter().filter(|(_, g)| g.transform.determinant() < 0.0).count();
            flipped > 0 && flipped < members.len()
        });
        MirrorSets(sets)
    }

    fn partners(&self, map: &Map, group: NodeId) -> Vec<(NodeId, AxisPlane)> {
        let Some(NodeKind::Group(g)) = map.get(group).map(|n| &n.kind) else { return Vec::new() };
        let Some(members) = g.link_id.and_then(|link| self.0.get(&link)) else { return Vec::new() };
        mirrors(group, g, members.iter().copied())
    }
}

/// The partner on `axis` whose plane is nearest `bounds`. In a row of touching pairs both neighbours of a half are
/// nearest, then a `selected` one wins, then the one that leaves an even number of halves beyond it to pair up among
/// themselves, then the lowest id.
fn nearest(
    found: &[(NodeId, AxisPlane)],
    axis: usize,
    bounds: &Aabb,
    selected: impl Fn(&NodeId) -> bool,
    meeting: impl Fn(NodeId) -> Vec<(NodeId, AxisPlane)>,
) -> Option<(NodeId, AxisPlane)> {
    let gap = |p: &AxisPlane| (bounds.min[axis] - p.at).max(p.at - bounds.max[axis]).max(0.0);
    let on_axis = found.iter().filter(|(_, p)| p.axis == axis);
    let least = on_axis.clone().map(|(_, p)| gap(p)).min_by(f64::total_cmp)?;
    let tied: Vec<(NodeId, AxisPlane)> = on_axis.filter(|(_, p)| gap(p) <= least + 1e-6).copied().collect();
    if let [one] = tied[..] {
        return Some(one);
    }

    tied.into_iter().min_by_key(|(o, p)| (!selected(o), beyond(*o, *p, &meeting) % 2 == 0, *o))
}

/// How many halves line up from `next` on, each meeting the one before across another plane on the axis of `plane`.
fn beyond(next: NodeId, plane: AxisPlane, meeting: impl Fn(NodeId) -> Vec<(NodeId, AxisPlane)>) -> usize {
    let (mut walked, mut across) = (vec![next], plane);
    while let Some((other, p)) =
        meeting(walked[walked.len() - 1]).into_iter().find(|(o, p)| p.axis == across.axis && (p.at - across.at).abs() >= 1e-6 && !walked.contains(o))
    {
        walked.push(other);
        across = p;
    }

    walked.len()
}

/// The `found` partners of `group` whose plane touches both halves within `dist`.
fn touching(map: &Map, group: NodeId, found: Vec<(NodeId, AxisPlane)>, dist: f64) -> Vec<(NodeId, AxisPlane)> {
    let bounds = map.bounds(group);
    found.into_iter().filter(|(other, plane)| plane.meets(&bounds, dist) && plane.meets(&map.bounds(*other), dist)).collect()
}

/// Mirror planes of the selected linked groups and of the linked groups around a selected node. Per group and axis only
/// the plane nearest the group counts, so in a row of mirrored pairs each pair shows its own plane rather than one
/// plane per far copy.
pub fn selection_planes(map: &Map, sel: &Selection) -> Vec<MirrorPlane> {
    let groups = linked_around(map, sel);
    // The walk of every map node waits until a selected group could have a mirror image at all.
    let sets = if groups.is_empty() { MirrorSets(HashMap::new()) } else { MirrorSets::of(map) };
    let mut planes: Vec<MirrorPlane> = Vec::new();
    for &group in &groups {
        let found = sets.partners(map, group);
        if found.is_empty() {
            continue;
        }

        let bounds = map.bounds(group);
        let meeting = |g: NodeId| touching(map, g, sets.partners(map, g), MIRROR_TOLERANCE);
        for axis in 0..3 {
            let Some((partner, plane)) = nearest(&found, axis, &bounds, |o| groups.contains(o), meeting) else { continue };
            let span = bounds.union(&map.bounds(partner));
            let drawn = planes.iter().any(|m| (m.group, m.partner) == (partner, group));
            if !span.is_empty() && !drawn {
                planes.push(MirrorPlane { plane, group, partner, span });
            }
        }
    }

    planes
}

/// The selected linked groups and the linked groups around a selected node.
fn linked_around(map: &Map, sel: &Selection) -> BTreeSet<NodeId> {
    // A node seen before had its ancestors walked already, so a big selection visits each node once.
    let (mut seen, mut groups) = (HashSet::new(), BTreeSet::new());
    for id in &sel.nodes {
        let mut at = Some(*id);
        while let Some(node) = at.filter(|n| seen.insert(*n)) {
            if link_of(map, node).is_some() {
                groups.insert(node);
            }

            at = map.get(node).and_then(|n| n.parent);
        }
    }

    groups
}

/// A linked `group` whose `partner` was slid along the mirror `plane` between them, `slide` away from where it mirrors
/// `group`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drift {
    pub plane: AxisPlane,
    pub group: NodeId,
    pub partner: NodeId,
    pub slide: DVec3,
}

fn transform_of(map: &Map, id: NodeId) -> Option<DMat4> {
    match map.get(id).map(|n| &n.kind) {
        Some(NodeKind::Group(g)) => Some(g.transform),
        _ => None,
    }
}

/// Where `other` sits relative to `group`.
fn relation(map: &Map, group: NodeId, other: NodeId) -> Option<DMat4> {
    Some(transform_of(map, other)? * transform_of(map, group)?.inverse())
}

/// A partner with the plane between it and a group and its slide along that plane.
type SlidPartner = (NodeId, AxisPlane, DVec3);

/// The members of `group`'s link set placed as its mirror image, each with its plane and its slide along the plane.
fn slid_partners(map: &Map, sets: &BTreeMap<u64, Vec<NodeId>>, group: NodeId) -> Vec<SlidPartner> {
    let Some(members) = link_of(map, group).and_then(|link| sets.get(&link)) else { return Vec::new() };
    members
        .iter()
        .copied()
        .filter(|m| *m != group)
        .filter_map(|m| relation(map, group, m).filter(|r| r.determinant() < 0.0).and_then(|r| AxisPlane::of_slid_reflection(&r)).map(|(p, s)| (m, p, s)))
        .collect()
}

/// The partner on `axis` in `found` whose plane is nearest `bounds`, one that is not slid first on a tie, then the
/// lower id. Returns it with whether it is slid.
fn nearest_slid(found: &[SlidPartner], bounds: &Aabb, axis: usize) -> Option<(NodeId, bool)> {
    let gap = |p: &AxisPlane| (bounds.min[axis] - p.at).max(p.at - bounds.max[axis]).max(0.0);
    let slid = |s: &DVec3| s.abs().max_element() > AxisPlane::SLIDE;
    found
        .iter()
        .filter(|(_, p, _)| p.axis == axis)
        .min_by(|a, b| {
            let (ga, gb) = (gap(&a.1), gap(&b.1));
            if (ga - gb).abs() > 1e-6 { ga.total_cmp(&gb) } else { (slid(&a.2), a.0).cmp(&(slid(&b.2), b.0)) }
        })
        .map(|(m, _, s)| (*m, slid(s)))
}

/// The linked groups around the selection whose nearest mirror partner on an axis was slid along the plane, while
/// that partner has no nearer or equally near partner that still mirrors it. The half that is selected or holds the
/// selection is the `group` that stays, the lower id when both are.
pub fn drifted(map: &Map, sel: &Selection) -> Vec<Drift> {
    let groups = linked_around(map, sel);
    if groups.is_empty() {
        return Vec::new();
    }

    let sets = linked::link_sets(map);
    let mut found: HashMap<NodeId, (Aabb, Vec<SlidPartner>)> = HashMap::new();
    let mut nearest = |group: NodeId, axis: usize| {
        let (bounds, partners) = found.entry(group).or_insert_with(|| (map.bounds(group), slid_partners(map, &sets, group)));
        nearest_slid(partners, bounds, axis)
    };
    let mut drifts: Vec<Drift> = Vec::new();
    for &group in &groups {
        for axis in 0..3 {
            let Some((partner, true)) = nearest(group, axis) else { continue };
            if nearest(partner, axis).is_some_and(|(_, slid)| !slid) {
                continue;
            }

            let stay = if groups.contains(&partner) { group.min(partner) } else { group };
            let moving = if stay == group { partner } else { group };
            if drifts.iter().any(|d| [d.group, d.partner].contains(&stay) && [d.group, d.partner].contains(&moving)) {
                continue;
            }

            let Some((plane, slide)) = relation(map, stay, moving).and_then(|r| AxisPlane::of_slid_reflection(&r)) else { continue };
            drifts.push(Drift { plane, group: stay, partner: moving, slide });
        }
    }

    drifts
}

/// Whether a member at `relation` to the half that stays sits slid along the plane the same way as `drift`'s partner:
/// flipped or not on each axis, and moved by the slide on every axis it is not flipped on other than the plane's.
fn in_line(relation: &DMat4, drift: &Drift) -> bool {
    const LINEAR: f64 = 1e-6;
    let axis = drift.plane.axis;
    (0..3).all(|a| {
        let column = relation.col(a).truncate();
        let sign = column[a].signum();
        let mut unit = DVec3::ZERO;
        unit[a] = sign;
        let off = relation.w_axis[a] - drift.slide[a];
        (column - unit).abs().max_element() <= LINEAR
            && (a == axis || if sign < 0.0 { drift.slide[a].abs() <= AxisPlane::SLIDE } else { off.abs() <= AxisPlane::SLIDE })
    })
}

/// Why [`mirror_again`] moved nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgainError {
    /// Nothing around the selection was slid along its mirror plane.
    NotSlid,
    /// A member that would move is locked, or sits in a locked group or layer.
    Locked(NodeId),
}

/// Moves the partner of each [`drifted`] pair back across its plane from the half that stays, with every member of its
/// link set slid the same way, so a facade row follows as a whole while a copy placed elsewhere stays. Returns how many
/// groups moved, how far the first partner moved and its plane. Hidden members move with their half, a locked one
/// refuses the whole move before anything is touched.
pub fn mirror_again(map: &mut Map, sel: &Selection) -> Result<(usize, f64, AxisPlane), AgainError> {
    let drifts = drifted(map, sel);
    let first = *drifts.first().ok_or(AgainError::NotSlid)?;
    let mut sets = BTreeSet::new();
    let mut plan = Vec::new();
    for drift in drifts {
        let Some(link) = link_of(map, drift.group).filter(|l| sets.insert(*l)) else { continue };
        let mut moving = Selection::default();
        for m in linked::members(map, link) {
            if m != drift.group && relation(map, drift.group, m).is_some_and(|r| in_line(&r, &drift)) {
                moving.nodes.insert(m);
            }
        }

        plan.push((drift, moving));
    }

    if let Some(locked) = plan.iter().flat_map(|(_, moving)| &moving.nodes).find(|m| map.is_locked(**m)) {
        return Err(AgainError::Locked(*locked));
    }

    let opts = EditOptions { uv_lock: true, ..EditOptions::default() };
    let mut moved = 0;
    for (drift, moving) in plan {
        moved += moving.nodes.len();
        ops::translate_selection(map, &moving, -drift.slide, opts);
    }

    Ok((moved, first.slide.length(), first.plane))
}

/// The halves of `mirror` with their solid bounds, the one lower on the axis first, the group on a tie.
fn sides(map: &Map, mirror: &MirrorPlane) -> [(NodeId, Aabb); 2] {
    let axis = mirror.plane.axis;
    let [a, b] = [mirror.group, mirror.partner].map(|id| (id, map.solid_bounds(id)));
    if b.1.center()[axis] < a.1.center()[axis] { [b, a] } else { [a, b] }
}

/// Space between the brushes and meshes of the halves of `mirror` along its axis, negative where they overlap, to a
/// thousandth so the [`COVER_SLACK`] left at full overlap reads as none.
pub fn gap(map: &Map, mirror: &MirrorPlane) -> f64 {
    let axis = mirror.plane.axis;
    let [(_, low), (_, high)] = sides(map, mirror);
    ((high.min[axis] - low.max[axis]) * 1000.0).round() / 1000.0
}

/// The gap [`set_gap`] sets for `gap`, between minus the narrower half's width and [`MAX_GAP`], and whether the halves
/// have to move for it.
pub fn gap_target(map: &Map, mirror: &MirrorPlane, gap: f64) -> (f64, bool) {
    let (gap, offset) = gap_offset(&sides(map, mirror), mirror.plane.axis, gap);
    (gap, offset != 0.0)
}

fn gap_offset(sides: &[(NodeId, Aabb); 2], axis: usize, gap: f64) -> (f64, f64) {
    let [(_, a), (_, b)] = sides;
    let width = |b: &Aabb| b.max[axis] - b.min[axis];
    let narrower = width(a).min(width(b));
    let gap = gap.max(-narrower).min(MAX_GAP);
    let offset = (gap.max(COVER_SLACK - narrower) - (b.min[axis] - a.max[axis])) / 2.0;
    (gap, if offset.abs() < 1e-9 { 0.0 } else { offset })
}

/// The members of the link set of `mirror` that [`set_gap`] moves, the halves themselves when it has no set.
fn gap_members(map: &Map, mirror: &MirrorPlane) -> Vec<NodeId> {
    let [(low, _), (high, _)] = sides(map, mirror);
    link_of(map, mirror.group).map(|link| linked::members(map, link)).unwrap_or_else(|| vec![low, high])
}

/// The first locked member [`set_gap`] would have to move to reach `gap`, none when the halves already are that far
/// apart. A member in a locked group or layer counts as locked.
pub fn gap_locked(map: &Map, mirror: &MirrorPlane, gap: f64) -> Option<NodeId> {
    if !gap_target(map, mirror, gap).1 {
        return None;
    }

    gap_members(map, mirror).into_iter().find(|m| map.is_locked(*m))
}

/// Moves the halves of `mirror` evenly apart or together until they are `gap` apart, so the plane stays. The rest of
/// the link set moves with the half on its side, which keeps every other mirror plane of the set a pure reflection,
/// hidden members included. A locked member refuses the move, nothing moves and it is returned as the error. The gap
/// stops at [`MAX_GAP`] and at minus the narrower half's width, where one half covers the other, less
/// [`COVER_SLACK`]. Returns the gap it set.
pub fn set_gap(map: &mut Map, mirror: &MirrorPlane, gap: f64) -> Result<f64, NodeId> {
    let axis = mirror.plane.axis;
    let sides = sides(map, mirror);
    let [(low, _), (high, _)] = sides;
    let (gap, by) = gap_offset(&sides, axis, gap);
    if by == 0.0 {
        return Ok(gap);
    }

    let members = gap_members(map, mirror);
    if let Some(locked) = members.iter().find(|m| map.is_locked(**m)) {
        return Err(*locked);
    }

    let mut offset = DVec3::ZERO;
    offset[axis] = by;
    let (mut lows, mut highs) = (Selection::default(), Selection::default());
    for m in members {
        let below = m == low || (m != high && map.solid_bounds(m).center()[axis] < mirror.plane.at);
        let side = if below { &mut lows } else { &mut highs };
        side.nodes.insert(m);
    }

    let opts = EditOptions { uv_lock: true, ..EditOptions::default() };
    ops::translate_selection(map, &lows, -offset, opts);
    ops::translate_selection(map, &highs, offset, opts);
    Ok(gap)
}

/// What [`apply_mirror`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The group each set of meeting halves was gathered into.
    pub groups: Vec<NodeId>,
    /// Brushes that went into a convex union with their mirror images.
    pub joined: usize,
    /// Meshes joined with their mirror images and welded along the planes.
    pub welded: usize,
    pub replaced: Replaced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// No selected group, or group around a selected node, has a mirror image in its link set.
    NotMirrored,
    /// Every mirror plane misses the halves, which then are copies placed apart rather than halves of one thing.
    Apart,
}

/// Bakes the selected mirrored halves into plain geometry. Halves at most `dist` apart across a plane are gathered,
/// transitively, into the lowest id one, which is unlinked and selected. Brushes at the same place in each half join
/// where their union is convex, meshes are joined and welded along the planes, and the rest moves over as it is.
pub fn apply_mirror(map: &mut Map, sel: &mut Selection, dist: f64) -> Result<Applied, ApplyError> {
    let around = |id: NodeId| std::iter::once(id).chain(map.ancestors(id));
    let linked: BTreeSet<NodeId> = sel.nodes.iter().flat_map(|id| around(*id)).filter(|g| link_of(map, *g).is_some()).collect();
    let mirrored = linked.iter().any(|g| !partners(map, *g).is_empty());
    let meets: BTreeSet<NodeId> = linked.into_iter().filter(|g| !meeting(map, *g, dist).is_empty()).collect();
    // The innermost halves around each selected node, so windows mirrored inside a mirrored house apply on their own.
    let seeds: BTreeSet<NodeId> = sel.nodes.iter().filter_map(|id| around(*id).find(|g| meets.contains(g))).collect();

    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    let mut found: Vec<(Vec<NodeId>, Vec<AxisPlane>)> = Vec::new();
    for &seed in &seeds {
        if !seen.insert(seed) {
            continue;
        }

        let (mut members, mut planes) = (vec![seed], Vec::<AxisPlane>::new());
        let mut next = 0;
        while next < members.len() {
            for other in follow(map, members[next], &mut planes, &seeds, &seen, dist) {
                if seen.insert(other) {
                    members.push(other);
                }
            }

            next += 1;
        }

        // A half whose partners all went to other pairs stays linked rather than being unlinked alone.
        if members.len() > 1 {
            members.sort();
            found.push((members, planes));
        }
    }

    if found.is_empty() {
        return Err(if mirrored { ApplyError::Apart } else { ApplyError::NotMirrored });
    }

    let inside = |a: &[NodeId], b: &[NodeId]| a.iter().any(|m| b.iter().any(|g| map.is_ancestor(*g, *m)));
    let outer: Vec<usize> = (0..found.len()).filter(|i| !found.iter().any(|(b, _)| inside(&found[*i].0, b))).collect();
    let mut applied = Applied::default();
    for i in outer {
        let (members, planes) = &found[i];
        gather(map, members, planes, dist, &mut applied);
    }

    sel.clear();
    sel.nodes.extend(&applied.groups);
    Ok(applied)
}

/// The meeting partners of `group` to gather with it. Per axis the first plane is the one to the [`nearest`] partner no
/// other pair took, and from then on only partners across that same plane follow, which gathers the XZ quarters of a
/// pair but not the touching pairs of a tiled row, whose halves also mirror each other across the next plane over.
fn follow(map: &Map, group: NodeId, planes: &mut Vec<AxisPlane>, seeds: &BTreeSet<NodeId>, seen: &BTreeSet<NodeId>, dist: f64) -> Vec<NodeId> {
    let (found, bounds) = (meeting(map, group, dist), map.bounds(group));
    let open: Vec<(NodeId, AxisPlane)> = found.iter().copied().filter(|(o, _)| !seen.contains(o)).collect();
    let mut out = Vec::new();
    for axis in 0..3 {
        if let Some(used) = planes.iter().find(|p| p.axis == axis) {
            out.extend(found.iter().filter(|(_, p)| p.axis == axis && (p.at - used.at).abs() < 1e-6).map(|(o, _)| *o));
        } else if let Some((other, plane)) = nearest(&open, axis, &bounds, |o| seeds.contains(o), |g| meeting(map, g, dist)) {
            planes.push(plane);
            out.push(other);
        }
    }

    out
}

/// Mirror partners of `group` at most `dist` apart across their plane, the gap the Inspector shows.
fn meeting(map: &Map, group: NodeId, dist: f64) -> Vec<(NodeId, AxisPlane)> {
    touching(map, group, partners(map, group), dist / 2.0)
}

/// Kind, depth and link of every descendant in depth-first order, which sync keeps equal across a link set.
fn shape(map: &Map, id: NodeId, depth: usize, out: &mut Vec<(usize, &'static str, bool)>) {
    for c in map.get(id).map(|n| n.children.as_slice()).unwrap_or_default() {
        let Some(child) = map.get(*c) else { continue };
        out.push((depth, child.kind.type_name(), link_of(map, *c).is_some()));
        shape(map, *c, depth + 1, out);
    }
}

/// Brushes and meshes reached through plain groups only. Nested linked groups keep their own sets in sync and brush
/// entities their brushes, so those move over whole.
fn mergeable(map: &Map, group: NodeId, out: &mut Vec<NodeId>) {
    for c in map.get(group).map(|n| n.children.as_slice()).unwrap_or_default() {
        match map.get(*c).map(|n| &n.kind) {
            Some(NodeKind::Brush(_) | NodeKind::Mesh(_)) => out.push(*c),
            Some(NodeKind::Group(g)) if g.link_id.is_none() => mergeable(map, *c, out),
            _ => {}
        }
    }
}

/// Moves the contents of `members` into the first one and unlinks it. Halves an older release let drift apart in
/// structure are only gathered, since their leaves no longer pair up by position.
fn gather(map: &mut Map, members: &[NodeId], planes: &[AxisPlane], dist: f64, applied: &mut Applied) {
    let keep = members[0];
    let shapes: Vec<_> = members
        .iter()
        .map(|m| {
            let mut out = Vec::new();
            shape(map, *m, 0, &mut out);
            out
        })
        .collect();
    if shapes.iter().all(|s| *s == shapes[0]) {
        let leaves: Vec<Vec<NodeId>> = members
            .iter()
            .map(|m| {
                let mut out = Vec::new();
                mergeable(map, *m, &mut out);
                out
            })
            .collect();
        for i in 0..leaves[0].len() {
            let nodes: Vec<NodeId> = leaves.iter().map(|l| l[i]).collect();
            merge_leaves(map, &nodes, planes, dist, applied);
        }
    }

    for &other in &members[1..] {
        for c in map.get(other).map(|n| n.children.clone()).unwrap_or_default() {
            map.reparent(c, keep);
        }

        map.remove(other);
        applied.replaced.insert(other, vec![keep]);
    }

    if let Some(NodeKind::Group(g)) = map.get_mut(keep).map(|n| &mut n.kind) {
        g.link_id = None;
        g.transform = DMat4::IDENTITY;
    }

    applied.groups.push(keep);
}

/// Joins the brushes or meshes at one place in each half. The first nodes take the result, the rest are removed along
/// with the plain groups they leave empty.
fn merge_leaves(map: &mut Map, nodes: &[NodeId], planes: &[AxisPlane], dist: f64, applied: &mut Applied) {
    let kept = match map.get(nodes[0]).map(|n| &n.kind) {
        Some(NodeKind::Brush(_)) => {
            let pieces: Vec<Brush> = nodes.iter().filter_map(|n| map.brush(*n).cloned()).collect();
            let joined = csg::join_convex(pieces);
            let kept = joined.len();
            for (n, brush) in nodes.iter().zip(joined) {
                if let Some(slot) = map.brush_mut(*n) {
                    *slot = brush;
                }
            }

            applied.joined += nodes.len() - kept;
            kept
        }
        Some(NodeKind::Mesh(first)) => {
            let mut mesh = first.clone();
            for n in &nodes[1..] {
                if let Some(other) = map.mesh(*n) {
                    mesh.join(other);
                }
            }

            for plane in planes {
                mesh.weld_seam(*plane, dist);
            }

            if let Some(slot) = map.mesh_mut(nodes[0]) {
                *slot = mesh;
            }

            applied.welded += 1;
            1
        }
        _ => return,
    };

    let survivors = nodes[..kept].to_vec();
    for &n in &nodes[kept..] {
        let mut up = map.get(n).and_then(|x| x.parent);
        map.remove(n);
        applied.replaced.insert(n, survivors.clone());
        while let Some(p) = up.filter(|p| map.get(*p).is_some_and(|n| n.children.is_empty() && matches!(&n.kind, NodeKind::Group(g) if g.link_id.is_none()))) {
            up = map.get(p).and_then(|x| x.parent);
            map.remove(p);
            applied.replaced.insert(p, Vec::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use td_core::DMat4;
    use td_geom::{Brush, Mesh};

    use super::*;
    use crate::linked::tests::{assert_copy_maps, wedge};
    use crate::map::Instance;
    use crate::scatter::ScatterInstance;
    use crate::symmetry::{MIRROR_TOLERANCE, PlaneAt};
    use crate::{Document, Entity, Scatter, ScatterItem, ScatterKind};

    const WEDGE_MIN: DVec3 = DVec3::new(8.0, 2.0, 4.0);
    const WEDGE_MAX: DVec3 = DVec3::new(40.0, 34.0, 20.0);

    fn group_of(doc: &mut Document, brush: Brush, name: &str) -> NodeId {
        let layer = doc.map.default_layer();
        doc.edit("add", |m, s| {
            s.clear();
            s.select_node(m.insert(layer, NodeKind::Brush(brush)));
            ops::group_selection(m, s, name, layer).unwrap()
        })
    }

    fn select(doc: &mut Document, ids: &[NodeId]) {
        doc.select(|_, s| {
            s.clear();
            s.nodes.extend(ids);
        });
    }

    fn mirror(doc: &mut Document, ids: &[NodeId], axis: usize, at: PlaneAt) -> (Vec<NodeId>, AxisPlane) {
        select(doc, ids);
        doc.edit("Mirror Linked", |m, s| mirror_linked(m, s, axis, &PlaneChoice { at, custom: 100.25 }, 0.0)).unwrap()
    }

    fn shifted(doc: &mut Document, ids: &[NodeId], offset: DVec3) -> Vec<NodeId> {
        select(doc, ids);
        doc.edit("link", |m, s| ops::duplicate_linked(m, s, offset, EditOptions::default()))
    }

    fn transform(map: &Map, id: NodeId) -> DMat4 {
        match &map.get(id).unwrap().kind {
            NodeKind::Group(g) => g.transform,
            _ => panic!("{id} is no group"),
        }
    }

    fn plane(axis: usize, at: f64) -> AxisPlane {
        AxisPlane { axis, at }
    }

    #[test]
    fn axis_plane_matrix_is_a_flip_about_a_point_on_it() {
        let point = DVec3::new(37.5, -12.25, 3.0);
        for axis in 0..3 {
            let at = AxisPlane { axis, at: point[axis] };
            assert!(at.matrix().abs_diff_eq(ops::flip_about(point, axis), 1e-12), "axis {axis}");
        }
    }

    #[test]
    fn mirror_linked_puts_the_copy_where_each_plane_choice_says() {
        let choices = [
            (PlaneAt::Center, (WEDGE_MIN + WEDGE_MAX) * 0.5),
            (PlaneAt::Max, WEDGE_MAX),
            (PlaneAt::Min, WEDGE_MIN),
            (PlaneAt::World, DVec3::ZERO),
            (PlaneAt::Custom, DVec3::splat(100.25)),
        ];
        for (at, want) in choices {
            for axis in 0..3 {
                let mut doc = Document::new();
                let group = group_of(&mut doc, wedge(), "half");
                let before = doc.map.nodes.clone();
                let (copies, found) = mirror(&mut doc, &[group], axis, at);
                assert_eq!(found, plane(axis, want[axis]), "{at:?}");
                assert_eq!(copies.len(), 1);
                let copy = copies[0];
                assert_eq!(doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [copy]);
                assert!(link_of(&doc.map, group).is_some());
                assert_copy_maps(&doc.map, group, copy, &found.matrix());
                assert_eq!(partners(&doc.map, group), [(copy, found)], "{at:?} on {axis}");
                assert_eq!(partners(&doc.map, copy), [(group, found)], "{at:?} on {axis}");

                assert_eq!(doc.undo().as_deref(), Some("Mirror Linked"));
                assert!(doc.map.nodes == before, "one undo step removes the copy and the link");
            }
        }
    }

    #[test]
    fn moving_one_half_keeps_the_plane_halfway_between_them() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let (copies, found) = mirror(&mut doc, &[group], 0, PlaneAt::Max);
        let copy = copies[0];
        let move_copy = |doc: &mut Document, offset: DVec3| {
            select(doc, &[copy]);
            doc.edit("move", |m, s| ops::translate_selection(m, s, offset, EditOptions::default()));
            partners(&doc.map, group)
        };
        assert_eq!(move_copy(&mut doc, DVec3::new(64.0, 0.0, 0.0)), [(copy, plane(0, found.at + 32.0))]);
        assert_eq!(move_copy(&mut doc, DVec3::new(0.0, 8.0, 0.0)), [], "a slide along the plane is no plain mirror");
    }

    #[test]
    fn mirror_linked_needs_a_selected_group_and_a_plane() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let loose = doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(wedge())));
        let empty = doc.edit("empty", |m, _| m.insert(layer, NodeKind::Group(crate::Group::new("empty"))));
        let before = doc.map.nodes.clone();
        for (ids, at) in [(&[][..], PlaneAt::World), (&[loose][..], PlaneAt::World), (&[empty][..], PlaneAt::Max), (&[empty][..], PlaneAt::Center)] {
            select(&mut doc, ids);
            let mut sel = doc.selection.clone();
            assert_eq!(mirror_linked(&mut doc.map, &mut sel, 0, &PlaneChoice { at, custom: 0.0 }, 0.0), None, "{ids:?} {at:?}");
            assert!(doc.map.nodes == before, "{ids:?} {at:?} changed the map");
            assert_eq!(sel, doc.selection, "{ids:?} {at:?} changed the selection");
        }

        // The world origin needs no bounds, so even an empty group gets its mirrored twin.
        let (copies, found) = mirror(&mut doc, &[empty], 1, PlaneAt::World);
        assert_eq!((copies.len(), found), (1, plane(1, 0.0)));
        assert_eq!(partners(&doc.map, empty), [(copies[0], found)]);
        select(&mut doc, &[empty]);
        assert_eq!(selection_planes(&doc.map, &doc.selection), [], "an empty pair spans nothing to draw");
    }

    #[test]
    fn each_selected_group_gets_a_copy_and_loose_objects_stay() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let a = group_of(&mut doc, wedge(), "a");
        let post = Brush::from_aabb(&Aabb::new(DVec3::new(-24.0, 0.0, 30.0), DVec3::new(-16.0, 48.0, 38.0)), "trim").unwrap();
        let b = group_of(&mut doc, post, "b");
        let far = Brush::from_aabb(&Aabb::new(DVec3::splat(200.0), DVec3::splat(260.0)), "far").unwrap();
        let loose = doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(far)));
        let (copies, found) = mirror(&mut doc, &[a, b, loose], 2, PlaneAt::Max);
        assert_eq!(found, plane(2, 38.0), "the loose brush is not mirrored, so it does not move the plane");
        assert_eq!(copies.len(), 2);
        assert_eq!(doc.map.bounds(loose), Aabb::new(DVec3::splat(200.0), DVec3::splat(260.0)));
        let (la, lb) = (link_of(&doc.map, a).unwrap(), link_of(&doc.map, b).unwrap());
        assert_ne!(la, lb);
        assert_eq!([link_of(&doc.map, copies[0]), link_of(&doc.map, copies[1])], [Some(la), Some(lb)]);
        assert_copy_maps(&doc.map, a, copies[0], &found.matrix());
        assert_copy_maps(&doc.map, b, copies[1], &found.matrix());

        // Members of one set mirror into that set, each copy placed as its own original's reflection.
        let shift = DVec3::new(0.0, 0.0, -64.0);
        let moved = shifted(&mut doc, &[a], shift)[0];
        let (copies, found) = mirror(&mut doc, &[a, moved], 0, PlaneAt::Min);
        assert_eq!(found, plane(0, 8.0));
        assert_eq!(copies.iter().map(|c| link_of(&doc.map, *c)).collect::<Vec<_>>(), [Some(la), Some(la)]);
        assert!(transform(&doc.map, copies[0]).abs_diff_eq(found.matrix(), 1e-9));
        assert!(transform(&doc.map, copies[1]).abs_diff_eq(found.matrix() * DMat4::from_translation(shift), 1e-9));
        assert_copy_maps(&doc.map, moved, copies[1], &found.matrix());
    }

    #[test]
    fn partners_are_the_members_placed_as_a_reflection() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let plain = group_of(&mut doc, wedge(), "plain");
        assert_eq!(partners(&doc.map, group), []);
        assert_eq!(partners(&doc.map, plain), []);
        assert_eq!(partners(&doc.map, doc.map.get(group).unwrap().children[0]), [], "a brush has no partners");
        assert_eq!(partners(&doc.map, NodeId(9999)), []);

        let mirrored = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let moved = shifted(&mut doc, &[group], DVec3::new(64.0, 0.0, 0.0))[0];
        let slid = shifted(&mut doc, &[group], DVec3::new(0.0, 0.0, 96.0))[0];
        assert_eq!(partners(&doc.map, group), [(mirrored, plane(0, 40.0))]);
        assert_eq!(partners(&doc.map, moved), [(mirrored, plane(0, 72.0))], "x' = 80 - (x - 64)");
        assert_eq!(partners(&doc.map, slid), [], "a reflection plus a slide along the plane is no mirror");
        assert_eq!(partners(&doc.map, mirrored), [(group, plane(0, 40.0)), (moved, plane(0, 72.0))]);
        assert_eq!(partners(&doc.map, plain), []);

        select(&mut doc, &[mirrored]);
        let turn = ops::rotation_about(DVec3::new(56.0, 10.0, 12.0), DVec3::Y, 30.0);
        doc.edit("turn", |m, s| ops::transform_selection(m, s, &turn, EditOptions::default()));
        assert_eq!(partners(&doc.map, group), []);
        assert_eq!(partners(&doc.map, mirrored), []);
    }

    /// Three mirrored pairs along x, each copy of the first pair 96 units further: halves at 8..40 | 40..72,
    /// 104..136 | 136..168 and 200..232 | 232..264. Every left half also mirrors every right half across a far plane.
    fn facade(doc: &mut Document) -> [[NodeId; 2]; 3] {
        let a = group_of(doc, wedge(), "window");
        let b = mirror(doc, &[a], 0, PlaneAt::Max).0[0];
        let one = shifted(doc, &[a, b], DVec3::new(96.0, 0.0, 0.0));
        let two = shifted(doc, &[a, b], DVec3::new(192.0, 0.0, 0.0));
        [[a, b], [one[0], one[1]], [two[0], two[1]]]
    }

    fn span(min_x: f64, max_x: f64) -> Aabb {
        Aabb::new(DVec3::new(min_x, WEDGE_MIN.y, WEDGE_MIN.z), DVec3::new(max_x, WEDGE_MAX.y, WEDGE_MAX.z))
    }

    /// `count` halves of mirrored pairs placed end to end along x from x 8, each 32 wide, the last one alone when
    /// `count` is odd. Each half also mirrors the touching half of the next pair across their contact plane.
    fn touching_row(doc: &mut Document, count: usize) -> Vec<NodeId> {
        let a = group_of(doc, wedge(), "window");
        let mut row = vec![a, mirror(doc, &[a], 0, PlaneAt::Max).0[0]];
        while row.len() < count {
            let pair = row[..(count - row.len()).min(2)].to_vec();
            row.extend(shifted(doc, &pair, DVec3::new(32.0 * row.len() as f64, 0.0, 0.0)));
        }

        row
    }

    #[test]
    fn selection_planes_keep_the_nearest_partner_per_axis() {
        let mut doc = Document::new();
        let [[a, b], [c, d], [e, f]] = facade(&mut doc);
        assert_eq!(partners(&doc.map, c), [(b, plane(0, 88.0)), (d, plane(0, 136.0)), (f, plane(0, 184.0))]);
        let planes = |doc: &mut Document, ids: &[NodeId]| {
            select(doc, ids);
            selection_planes(&doc.map, &doc.selection)
        };
        assert_eq!(planes(&mut doc, &[c]), [MirrorPlane { plane: plane(0, 136.0), group: c, partner: d, span: span(104.0, 168.0) }]);
        assert_eq!(planes(&mut doc, &[f]), [MirrorPlane { plane: plane(0, 232.0), group: f, partner: e, span: span(200.0, 264.0) }]);
        assert_eq!(planes(&mut doc, &[c, d]), [MirrorPlane { plane: plane(0, 136.0), group: c, partner: d, span: span(104.0, 168.0) }], "a pair draws once");
        assert_eq!(
            planes(&mut doc, &[a, e]),
            [
                MirrorPlane { plane: plane(0, 40.0), group: a, partner: b, span: span(8.0, 72.0) },
                MirrorPlane { plane: plane(0, 232.0), group: e, partner: f, span: span(200.0, 264.0) },
            ]
        );

        let leaf = doc.map.get(d).unwrap().children[0];
        assert_eq!(planes(&mut doc, &[leaf]), [MirrorPlane { plane: plane(0, 136.0), group: d, partner: c, span: span(104.0, 168.0) }], "inside an open half");
        assert_eq!(planes(&mut doc, &[]), []);
        let everything: Vec<NodeId> = doc.map.nodes.keys().copied().collect();
        let pairs = planes(&mut doc, &everything).iter().map(|m| (m.group, m.partner, m.plane)).collect::<Vec<_>>();
        assert_eq!(pairs, [(a, b, plane(0, 40.0)), (c, d, plane(0, 136.0)), (e, f, plane(0, 232.0))], "select all");
    }

    #[test]
    fn selection_planes_pair_touching_halves_the_way_apply_mirror_does() {
        let mut doc = Document::new();
        let [a, b, c, d, e, f]: [NodeId; 6] = touching_row(&mut doc, 6).try_into().unwrap();
        let mut planes = |ids: &[NodeId]| {
            select(&mut doc, ids);
            selection_planes(&doc.map, &doc.selection).iter().map(|m| (m.group, m.partner, m.plane)).collect::<Vec<_>>()
        };
        assert_eq!(planes(&[c]), [(c, d, plane(0, 104.0))]);
        assert_eq!(planes(&[d]), [(d, c, plane(0, 104.0))]);
        assert_eq!(planes(&[b, c]), [(b, c, plane(0, 72.0))], "a selected neighbour wins");
        let all = planes(&[a, b, c, d, e, f]);
        assert_eq!(all, [(a, b, plane(0, 40.0)), (c, d, plane(0, 104.0)), (e, f, plane(0, 168.0))]);
    }

    #[test]
    fn quarters_show_both_planes_and_turned_or_plain_groups_none() {
        let mut doc = Document::new();
        let a = group_of(&mut doc, wedge(), "quarter");
        let b = mirror(&mut doc, &[a], 0, PlaneAt::Max).0[0];
        let [c, d]: [NodeId; 2] = mirror(&mut doc, &[a, b], 2, PlaneAt::Min).0.try_into().unwrap();
        let quarter = |min: DVec3, max: DVec3| Aabb::new(min, max);
        let whole = |doc: &Document| selection_planes(&doc.map, &doc.selection);
        select(&mut doc, &[a]);
        assert_eq!(
            whole(&doc),
            [
                MirrorPlane { plane: plane(0, 40.0), group: a, partner: b, span: quarter(WEDGE_MIN, DVec3::new(72.0, 34.0, 20.0)) },
                MirrorPlane { plane: plane(2, 4.0), group: a, partner: c, span: quarter(DVec3::new(8.0, 2.0, -12.0), WEDGE_MAX) },
            ]
        );
        assert_eq!(partners(&doc.map, d).iter().map(|(id, p)| (*id, p.axis)).collect::<Vec<_>>(), [(b, 2), (c, 0)], "a is half a turn away from d");

        let plain = group_of(&mut doc, wedge(), "plain");
        let turned = shifted(&mut doc, &[plain], DVec3::ZERO)[0];
        let turn = ops::rotation_about(DVec3::new(40.0, 0.0, 4.0), DVec3::Y, 90.0);
        doc.edit("turn", |m, s| ops::transform_selection(m, s, &turn, EditOptions::default()));
        select(&mut doc, &[plain, turned]);
        assert_eq!(whole(&doc), []);
        doc.edit("unlink", |m, s| ops::unlink_groups(m, s));
        assert_eq!(whole(&doc), []);
    }

    /// A closed block from x 8 to 24 whose top slopes down along x and up along z, so it touches the wedge's min side.
    fn ramp() -> Mesh {
        let points = [
            DVec3::new(8.0, 40.0, 6.0),
            DVec3::new(24.0, 40.0, 6.0),
            DVec3::new(8.0, 40.0, 20.0),
            DVec3::new(24.0, 40.0, 20.0),
            DVec3::new(8.0, 56.0, 6.0),
            DVec3::new(8.0, 59.5, 20.0),
            DVec3::new(24.0, 48.0, 6.0),
            DVec3::new(24.0, 51.5, 20.0),
        ];
        Mesh::from_brush(&Brush::from_points(&points, &[], "plaster").unwrap())
    }

    const LAMP: DVec3 = DVec3::new(20.0, 10.0, 12.0);

    /// A group holding the wedge, the ramp and a light, in that order.
    fn furnished(doc: &mut Document) -> NodeId {
        let layer = doc.map.default_layer();
        doc.edit("add", |m, s| {
            s.clear();
            let mut light = Entity::new("light");
            light.origin = LAMP;
            for kind in [NodeKind::Brush(wedge()), NodeKind::Mesh(ramp()), NodeKind::Entity(light)] {
                s.select_node(m.insert(layer, kind));
            }

            ops::group_selection(m, s, "half", layer).unwrap()
        })
    }

    fn children(map: &Map, id: NodeId) -> Vec<NodeId> {
        map.get(id).unwrap().children.clone()
    }

    fn of_kind(map: &Map, group: NodeId, kind: &str) -> Vec<NodeId> {
        children(map, group).into_iter().filter(|c| map.get(*c).unwrap().kind.type_name() == kind).collect()
    }

    fn apply(doc: &mut Document, ids: &[NodeId], dist: f64) -> Result<Applied, ApplyError> {
        select(doc, ids);
        doc.try_edit("Apply Mirror", |m, s| apply_mirror(m, s, dist))
    }

    fn assert_unlinked(map: &Map, group: NodeId, name: &str) {
        let NodeKind::Group(g) = &map.get(group).unwrap().kind else { panic!("{group} is no group") };
        assert_eq!((g.name.as_str(), g.link_id, g.transform), (name, None, DMat4::IDENTITY));
    }

    fn assert_points(found: &[DVec3], want: &[DVec3]) {
        assert_eq!(found.len(), want.len(), "{found:?}");
        for p in want {
            assert!(found.iter().any(|q| q.abs_diff_eq(*p, 1e-6)), "{p} missing from {found:?}");
        }
    }

    fn assert_bounds(found: Aabb, want: Aabb) {
        assert!(found.min.abs_diff_eq(want.min, 1e-9) && found.max.abs_diff_eq(want.max, 1e-9), "{found:?}, wants {want:?}");
    }

    fn assert_no_face_on(mesh: &Mesh, plane: AxisPlane) {
        let on = |f: &&td_geom::MeshFace| f.indices.iter().all(|v| plane.distance(mesh.vertices[*v as usize]).abs() < 1e-9);
        assert_eq!(mesh.faces.iter().filter(on).count(), 0, "a cap is left on {plane:?}");
    }

    #[test]
    fn applying_a_pair_joins_the_brush_welds_the_mesh_and_keeps_both_lights() {
        let mut doc = Document::new();
        let group = furnished(&mut doc);
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Min).0[0];
        let [brush, mesh, light]: [NodeId; 3] = children(&doc.map, group).try_into().unwrap();
        let [copy_brush, copy_mesh, copy_light]: [NodeId; 3] = children(&doc.map, copy).try_into().unwrap();
        let before = doc.map.nodes.clone();

        let applied = apply(&mut doc, &[copy], MIRROR_TOLERANCE).unwrap();
        let replaced = Replaced::from([(copy, vec![group]), (copy_brush, vec![brush]), (copy_mesh, vec![mesh])]);
        assert_eq!(applied, Applied { groups: vec![group], joined: 1, welded: 1, replaced });
        assert!(!doc.map.contains(copy));
        assert_unlinked(&doc.map, group, "half");
        assert_eq!(children(&doc.map, group), [brush, mesh, light, copy_light]);
        assert_eq!(doc.selection.nodes.iter().copied().collect::<Vec<_>>(), [group]);

        let joined = doc.map.brush(brush).unwrap();
        joined.validate().unwrap();
        assert_eq!(joined.faces.len(), 7);
        assert!(joined.faces.iter().all(|f| f.data.material == "wall"));
        let end = |x: f64| [DVec3::new(x, 2.0, 4.0), DVec3::new(x, 2.0, 20.0), DVec3::new(x, 14.0, 4.0), DVec3::new(x, 18.0, 20.0)];
        let ridge = [DVec3::new(8.0, 30.0, 4.0), DVec3::new(8.0, 34.0, 20.0)];
        assert_points(&joined.vertices, &[end(40.0).as_slice(), &end(-24.0), &ridge].concat());

        let welded = doc.map.mesh(mesh).unwrap();
        assert!(welded.is_closed());
        assert_eq!((welded.vertices.len(), welded.faces.len()), (12, 10));
        assert_bounds(welded.bounds(), Aabb::new(DVec3::new(-8.0, 40.0, 6.0), DVec3::new(24.0, 59.5, 20.0)));
        assert_no_face_on(welded, plane(0, 8.0));

        let origins = [light, copy_light].map(|l| doc.map.entity(l).unwrap().origin);
        assert_eq!(origins, [LAMP, DVec3::new(-4.0, 10.0, 12.0)]);

        assert_eq!(doc.undo().as_deref(), Some("Apply Mirror"));
        assert!(doc.map.nodes == before, "one undo step brings the linked pair back");
    }

    #[test]
    fn applying_one_pair_of_a_facade_leaves_the_other_pairs_linked() {
        let mut doc = Document::new();
        let [[a, b], [c, d], [e, f]] = facade(&mut doc);
        let link = link_of(&doc.map, a);
        let leaves = [c, d].map(|g| children(&doc.map, g)[0]);
        let shapes = leaves.map(|id| doc.map.brush(id).unwrap().clone());

        let applied = apply(&mut doc, &[d], MIRROR_TOLERANCE).unwrap();
        assert_eq!(applied, Applied { groups: vec![c], replaced: Replaced::from([(d, vec![c])]), ..Applied::default() });
        assert_eq!(children(&doc.map, c), leaves, "the halves meet in a valley, so their union is not convex");
        assert_eq!(leaves.map(|id| doc.map.brush(id).unwrap().clone()), shapes);
        assert_unlinked(&doc.map, c, "window");
        assert!(!doc.map.contains(d));
        assert_eq!([a, b, e, f].map(|g| link_of(&doc.map, g)), [link; 4]);
        assert_eq!(partners(&doc.map, a), [(b, plane(0, 40.0)), (f, plane(0, 136.0))]);
        assert_eq!(partners(&doc.map, e), [(b, plane(0, 136.0)), (f, plane(0, 232.0))]);
    }

    #[test]
    fn applying_touching_pairs_gathers_each_half_with_its_own_pair() {
        // Halves in the row, selected and pairs gathered into their first half, as indices into the row.
        type Case = (usize, &'static [usize], &'static [[usize; 2]]);
        let cases: [Case; 9] = [
            (6, &[3], &[[2, 3]]),
            (6, &[2], &[[2, 3]]),
            (6, &[1], &[[0, 1]]),
            (6, &[2, 3], &[[2, 3]]),
            (6, &[4, 5], &[[4, 5]]),
            (6, &[0, 1, 2, 3, 4, 5], &[[0, 1], [2, 3], [4, 5]]),
            (6, &[1, 2], &[[1, 2]]),
            (5, &[2], &[[1, 2]]),
            (5, &[0, 1, 2, 3, 4], &[[0, 1], [2, 3]]),
        ];
        for (count, picked, pairs) in cases {
            let mut doc = Document::new();
            let row = touching_row(&mut doc, count);
            let link = link_of(&doc.map, row[0]);
            let before: Vec<_> = row.iter().map(|g| (children(&doc.map, *g), doc.map.bounds(*g))).collect();
            let ids: Vec<NodeId> = picked.iter().map(|i| row[*i]).collect();

            let applied = apply(&mut doc, &ids, MIRROR_TOLERANCE).unwrap();
            let case = format!("{count} halves, {picked:?} selected");
            assert_eq!(applied.groups, pairs.iter().map(|[k, _]| row[*k]).collect::<Vec<_>>(), "{case}");
            for (i, half) in row.iter().enumerate() {
                match pairs.iter().find(|p| p.contains(&i)) {
                    Some([k, o]) if *k == i => {
                        assert_unlinked(&doc.map, *half, "window");
                        assert_bounds(doc.map.bounds(*half), before[*k].1.union(&before[*o].1));
                    }
                    Some(_) => assert!(!doc.map.contains(*half), "{case}: half {i} is left"),
                    None => {
                        let now = (link_of(&doc.map, *half), children(&doc.map, *half), doc.map.bounds(*half));
                        assert_eq!(now, (link, before[i].0.clone(), before[i].1), "{case}: half {i} changed");
                    }
                }
            }
        }
    }

    #[test]
    fn halves_must_meet_within_the_distance() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        select(&mut doc, &[group]);
        let gap = PlaneChoice { at: PlaneAt::Custom, custom: 40.5 };
        let copy = doc.edit("Mirror Linked", |m, s| mirror_linked(m, s, 0, &gap, 0.0)).unwrap().0[0];
        let before = doc.map.nodes.clone();
        assert_eq!(apply(&mut doc, &[group], 0.99), Err(ApplyError::Apart), "the halves are one unit apart");
        assert!(doc.map.nodes == before);
        assert_eq!(doc.history.undo_labels().next(), Some("Mirror Linked"), "a refused apply records no step");

        let brushes = [group, copy].map(|g| children(&doc.map, g)[0]);
        let applied = apply(&mut doc, &[group], 1.0).unwrap();
        assert_eq!(applied, Applied { groups: vec![group], replaced: Replaced::from([(copy, vec![group])]), ..Applied::default() });
        assert_eq!(children(&doc.map, group), brushes, "a gap of one unit keeps the brushes apart");

        let far = group_of(&mut doc, wedge(), "far");
        mirror(&mut doc, &[far], 2, PlaneAt::Custom);
        let before = doc.map.nodes.clone();
        assert_eq!(apply(&mut doc, &[far], 16.0), Err(ApplyError::Apart), "the plane at z 100.25 misses both halves");
        assert!(doc.map.nodes == before);
    }

    #[test]
    fn apply_needs_a_mirrored_linked_group_around_the_selection() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let loose = doc.edit("add", |m, _| m.insert(layer, NodeKind::Brush(wedge())));
        let solo = group_of(&mut doc, wedge(), "solo");
        let slid = group_of(&mut doc, wedge(), "slid");
        let next = shifted(&mut doc, &[slid], DVec3::new(32.0, 0.0, 0.0))[0];
        let half = group_of(&mut doc, wedge(), "half");
        let turned = mirror(&mut doc, &[half], 0, PlaneAt::Max).0[0];
        let turn = ops::rotation_about(DVec3::new(40.0, 10.0, 12.0), DVec3::Y, 90.0);
        doc.edit("turn", |m, s| ops::transform_selection(m, s, &turn, EditOptions::default()));
        let before = doc.map.nodes.clone();
        let inside = children(&doc.map, next)[0];
        for ids in [&[][..], &[loose][..], &[solo][..], &[slid, inside][..], &[half, turned][..]] {
            assert_eq!(apply(&mut doc, ids, MIRROR_TOLERANCE), Err(ApplyError::NotMirrored), "{ids:?}");
            assert!(doc.map.nodes == before, "{ids:?}");
        }
    }

    #[test]
    fn halves_out_of_sync_are_gathered_without_merging() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Min).0[0];
        let mut map = doc.map.clone();
        let post = Brush::from_aabb(&Aabb::new(DVec3::new(-20.0, 0.0, 30.0), DVec3::new(-12.0, 48.0, 38.0)), "trim").unwrap();
        // An older release could leave a brush in one half only.
        let extra = map.insert(copy, NodeKind::Brush(post.clone()));
        let brushes = [group, copy].map(|g| children(&map, g)[0]);
        let shapes = brushes.map(|id| map.brush(id).unwrap().clone());
        let mut sel = Selection::default();
        sel.select_node(group);

        let applied = apply_mirror(&mut map, &mut sel, MIRROR_TOLERANCE).unwrap();
        assert_eq!(applied, Applied { groups: vec![group], replaced: Replaced::from([(copy, vec![group])]), ..Applied::default() });
        assert_eq!(children(&map, group), [brushes[0], brushes[1], extra]);
        let [x, y] = shapes;
        assert_eq!([brushes[0], brushes[1], extra].map(|id| map.brush(id).unwrap().clone()), [x, y, post], "the halves would have joined");
        assert_unlinked(&map, group, "half");
        assert!(!map.contains(copy));
        assert_eq!(sel.nodes.iter().copied().collect::<Vec<_>>(), [group]);
    }

    #[test]
    fn nested_linked_windows_move_over_unchanged_and_stay_one_set() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let frame = Brush::from_aabb(&Aabb::new(DVec3::new(14.0, 10.0, 20.0), DVec3::new(22.0, 18.0, 23.0)), "frame").unwrap();
        let window = group_of(&mut doc, frame, "window");
        let twin = shifted(&mut doc, &[window], DVec3::new(10.0, 0.0, -2.0))[0];
        let half = doc.edit("house", |m, s| {
            s.clear();
            s.select_node(m.insert(layer, NodeKind::Brush(wedge())));
            let wall = ops::group_selection(m, s, "wall", layer).unwrap();
            s.clear();
            s.nodes.extend([wall, window, twin]);
            ops::group_selection(m, s, "half", layer).unwrap()
        });
        let copy = mirror(&mut doc, &[half], 0, PlaneAt::Min).0[0];
        let windows = |map: &Map, g| children(map, g).into_iter().filter(|c| link_of(map, *c).is_some()).collect::<Vec<_>>();
        let all = [windows(&doc.map, half), windows(&doc.map, copy)].concat();
        let snapshot = |map: &Map| {
            all.iter().flat_map(|w| [*w, children(map, *w)[0]]).map(|id| map.get(id).map(|n| (n.kind.clone(), n.children.clone())).unwrap()).collect::<Vec<_>>()
        };
        let placed = snapshot(&doc.map);
        let plain = |map: &Map, g| children(map, g).into_iter().filter(|c| link_of(map, *c).is_none()).collect::<Vec<_>>();
        let walls = [half, copy].map(|g| plain(&doc.map, g)[0]);
        let bricks = walls.map(|w| children(&doc.map, w)[0]);

        let applied = apply(&mut doc, &[half], MIRROR_TOLERANCE).unwrap();
        let replaced = Replaced::from([(copy, vec![half]), (bricks[1], vec![bricks[0]]), (walls[1], Vec::new())]);
        assert_eq!(applied, Applied { groups: vec![half], joined: 1, welded: 0, replaced });
        assert_eq!(windows(&doc.map, half), all);
        assert!(snapshot(&doc.map) == placed, "the windows keep their link, placement and panes");
        assert_eq!(plain(&doc.map, half), [walls[0]], "the wall group the merge emptied is gone");
        assert_eq!(children(&doc.map, walls[0]), [bricks[0]]);
        assert_eq!(doc.map.brush(bricks[0]).unwrap().bounds(), Aabb::new(DVec3::new(-24.0, 2.0, 4.0), DVec3::new(40.0, 34.0, 20.0)));

        let pane = children(&doc.map, window)[0];
        doc.edit("paint", |m, _| m.brush_mut(pane).unwrap().faces.iter_mut().for_each(|f| f.data.material = "glass".into()));
        for w in all {
            let b = doc.map.brush(children(&doc.map, w)[0]).unwrap();
            assert!(b.faces.iter().all(|f| f.data.material == "glass"), "{w} missed the edit");
        }
    }

    fn shifted_box(b: Aabb, axis: usize, by: f64) -> Aabb {
        let mut offset = DVec3::ZERO;
        offset[axis] = by;
        Aabb::new(b.min + offset, b.max + offset)
    }

    fn reflected(b: Aabb, p: AxisPlane) -> Aabb {
        Aabb::from_points([b.min, b.max].map(|c| p.matrix().transform_point3(c)))
    }

    fn the_plane(doc: &Document, ids: &[NodeId], axis: usize) -> MirrorPlane {
        let sel = Selection { nodes: ids.iter().copied().collect(), ..Selection::default() };
        selection_planes(&doc.map, &sel).into_iter().find(|m| m.plane.axis == axis).unwrap()
    }

    fn set(doc: &mut Document, mirror: &MirrorPlane, to: f64) -> f64 {
        let set = doc.edit("Set Mirror Gap", |m, _| set_gap(m, mirror, to)).unwrap();
        assert!(!doc.take_link_conflict(), "moving whole halves leaves the copies alike");
        set
    }

    fn rocks_at(position: DVec3) -> Scatter {
        let mut rocks = Scatter::new("rocks", ScatterKind::Props, vec![ScatterItem::new("res://a.glb")]);
        rocks.instances.push(ScatterInstance { item: 0, position, angles: DVec3::ZERO, scale: 1.0 });
        rocks
    }

    const WEDGE: Aabb = Aabb { min: WEDGE_MIN, max: WEDGE_MAX };

    #[test]
    fn solid_bounds_leave_out_scatter_entities_and_instances() {
        let mut doc = Document::new();
        let layer = doc.map.default_layer();
        let furniture = furnished(&mut doc);
        let [lamp] = of_kind(&doc.map, furniture, "entity")[..] else { panic!("one light") };
        let prop = Instance { path: "prop.tdm".into(), origin: DVec3::new(-50.0, 6.0, 30.0), angles: DVec3::ZERO, fixup: String::new() };
        let lone = doc.edit("add", |m, s| {
            m.insert(furniture, NodeKind::Scatter(rocks_at(DVec3::new(90.0, -20.0, 12.0))));
            m.insert(furniture, NodeKind::Instance(prop));
            let mut light = Entity::new("light");
            light.origin = LAMP;
            s.clear();
            s.select_node(m.insert(layer, NodeKind::Entity(light)));
            ops::group_selection(m, s, "lone", layer).unwrap()
        });

        assert_bounds(doc.map.solid_bounds(furniture), Aabb::new(WEDGE_MIN, DVec3::new(40.0, 59.5, 20.0)));
        assert_bounds(doc.map.bounds(furniture), Aabb::new(DVec3::new(-58.0, -20.0, -4.0), DVec3::new(106.0, 59.5, 38.0)));
        let [brush] = of_kind(&doc.map, furniture, "brush")[..] else { panic!("one brush") };
        assert_bounds(doc.map.solid_bounds(brush), WEDGE);
        let lamp_box = Aabb::from_center_size(LAMP, DVec3::splat(16.0));
        assert_bounds(doc.map.solid_bounds(lamp), lamp_box);
        assert_bounds(doc.map.solid_bounds(lone), lamp_box);
    }

    #[test]
    fn the_gap_counts_brushes_and_meshes_only() {
        let mut doc = Document::new();
        let half = furnished(&mut doc);
        doc.edit("add", |m, _| m.insert(half, NodeKind::Scatter(rocks_at(DVec3::new(60.0, 10.0, 12.0)))));
        let (copies, found) = mirror(&mut doc, &[half], 0, PlaneAt::Max);
        assert_eq!(found, plane(0, 76.0), "the plane is still put on the full bounds");

        let pair = the_plane(&doc, &[half], 0);
        assert_eq!((pair.group, pair.partner), (half, copies[0]));
        assert_eq!(gap(&doc.map, &pair), 72.0);
        assert!(found.meets(&doc.map.bounds(half), 0.0) && found.meets(&doc.map.bounds(copies[0]), 0.0), "the rocks of both halves touch");
    }

    #[test]
    fn setting_the_gap_moves_both_halves_evenly() {
        for axis in 0..3 {
            let mut doc = Document::new();
            let group = group_of(&mut doc, wedge(), "half");
            let (copies, found) = mirror(&mut doc, &[group], axis, PlaneAt::Max);
            let copy = copies[0];
            let picked = if axis == 1 { copy } else { group };
            let pair = the_plane(&doc, &[picked], axis);
            assert_eq!(pair.plane, found);
            assert_eq!(gap(&doc.map, &pair), 0.0);

            for to in [0.0, 1.0, 40.0] {
                assert_eq!(set(&mut doc, &pair, to), to, "axis {axis}");
                assert_eq!(gap(&doc.map, &pair), to, "axis {axis}");
                assert_eq!(partners(&doc.map, group), [(copy, found)], "the plane stays on {axis}");
                assert_bounds(doc.map.solid_bounds(group), shifted_box(WEDGE, axis, -to / 2.0));
                assert_bounds(doc.map.solid_bounds(copy), shifted_box(reflected(WEDGE, found), axis, to / 2.0));
                assert_copy_maps(&doc.map, group, copy, &found.matrix());
            }

            assert_eq!(doc.undo().as_deref(), Some("Set Mirror Gap"));
            assert_eq!(gap(&doc.map, &pair), 1.0);
        }
    }

    #[test]
    fn the_gap_clamps_at_minus_the_narrower_half() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let bystander = group_of(&mut doc, wedge(), "bystander");
        let pair = the_plane(&doc, &[group], 0);

        assert_eq!(set(&mut doc, &pair, -31.0), -31.0);
        assert_eq!(set(&mut doc, &pair, -32.0), -32.0, "exactly the width");
        assert_bounds(doc.map.solid_bounds(copy), shifted_box(doc.map.solid_bounds(group), 0, COVER_SLACK));
        let covered = doc.map.nodes.clone();
        assert_eq!(set(&mut doc, &pair, -33.0), -32.0, "past the width clamps");
        assert!(doc.map.nodes == covered, "a clamped gap that changes nothing moves nothing");
        assert_eq!(set(&mut doc, &pair, -100.0), -32.0);
        assert_eq!((gap(&doc.map, &pair), partners(&doc.map, group)), (-32.0, vec![(copy, plane(0, 40.0))]));
        assert_bounds(doc.map.solid_bounds(bystander), WEDGE);
        assert_eq!(gap_target(&doc.map, &pair, -100.0), (-32.0, false));
        assert_eq!(gap_target(&doc.map, &pair, 8.0), (8.0, true));
    }

    #[test]
    fn the_gap_stops_at_max_gap() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        mirror(&mut doc, &[group], 0, PlaneAt::Max);
        let pair = the_plane(&doc, &[group], 0);
        assert_eq!(set(&mut doc, &pair, 1e300), MAX_GAP);
        assert_eq!(gap(&doc.map, &pair), MAX_GAP);
        assert_eq!(gap_target(&doc.map, &pair, MAX_GAP + 1.0), (MAX_GAP, false));
        assert_eq!(gap_target(&doc.map, &pair, MAX_GAP), (MAX_GAP, false), "the same gap again moves nothing");
        assert_eq!(the_plane(&doc, &[group], 0).plane, plane(0, 40.0));
    }

    #[test]
    fn the_halves_keep_their_sides_through_full_overlap() {
        for picked_copy in [false, true] {
            let mut doc = Document::new();
            let group = group_of(&mut doc, wedge(), "half");
            let (copies, found) = mirror(&mut doc, &[group], 0, PlaneAt::Max);
            let copy = copies[0];
            let pair = the_plane(&doc, &[if picked_copy { copy } else { group }], 0);
            assert_eq!(set(&mut doc, &pair, -5000.0), -32.0);
            assert_eq!(gap(&doc.map, &the_plane(&doc, &[group], 0)), -32.0);
            let pair = the_plane(&doc, &[if picked_copy { copy } else { group }], 0);
            assert_eq!(set(&mut doc, &pair, 0.0), 0.0);
            assert_eq!(gap(&doc.map, &the_plane(&doc, &[group], 0)), 0.0);
            assert_bounds(doc.map.solid_bounds(group), WEDGE);
            assert_bounds(doc.map.solid_bounds(copy), reflected(WEDGE, found));
        }

        for at in [PlaneAt::Max, PlaneAt::Min] {
            let mut doc = Document::new();
            let group = group_of(&mut doc, wedge(), "half");
            select(&mut doc, &[group]);
            let choice = PlaneChoice { at, custom: 0.0 };
            let (copies, found) = doc.edit("Mirror Linked", |m, s| mirror_linked(m, s, 0, &choice, -5000.0)).unwrap();
            assert_bounds(doc.map.solid_bounds(group), WEDGE);
            assert_eq!(gap(&doc.map, &the_plane(&doc, &copies, 0)), -32.0, "{at:?}");
            let pair = the_plane(&doc, &copies, 0);
            set(&mut doc, &pair, 0.0);
            let (g, c) = (doc.map.solid_bounds(group), doc.map.solid_bounds(copies[0]));
            let low = if at == PlaneAt::Max { (g, c) } else { (c, g) };
            assert!((low.0.max.x - found.at).abs() < 1e-3 && (low.1.min.x - found.at).abs() < 1e-3, "{at:?} {g:?} {c:?} {found:?}");
        }
    }

    #[test]
    fn setting_the_gap_moves_a_hidden_member_and_refuses_a_locked_one() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let pair = the_plane(&doc, &[group], 0);
        with_flags(&mut doc, copy, true, false);
        let before = (doc.map.solid_bounds(group), doc.map.solid_bounds(copy));
        assert_eq!(gap_locked(&doc.map, &pair, 9.0), None);
        assert_eq!(set(&mut doc, &pair, 9.0), 9.0);
        assert_bounds(doc.map.solid_bounds(group), shifted_box(WEDGE, 0, -4.5));
        assert_eq!(doc.map.solid_bounds(copy).min.x - before.1.min.x, 4.5, "a hidden copy moves with its half");
        assert_eq!(doc.map.solid_bounds(group).min.x - before.0.min.x, -4.5);

        with_flags(&mut doc, copy, false, true);
        let (nodes, undo) = (doc.map.nodes.clone(), steps(&doc));
        assert_eq!(gap_locked(&doc.map, &pair, 20.0), Some(copy));
        assert_eq!(doc.try_edit("Set Mirror Gap", |m, _| set_gap(m, &pair, 20.0)), Err(copy));
        assert!(doc.map.nodes == nodes && steps(&doc) == undo, "nothing moves and no undo step is added");

        assert_eq!(gap_locked(&doc.map, &pair, 9.0), None, "a gap already set moves nothing, so the lock does not matter");
        assert_eq!(doc.try_edit("Set Mirror Gap", |m, _| set_gap(m, &pair, 9.0)), Ok(9.0));
        assert!(doc.map.nodes == nodes);
    }

    #[test]
    fn setting_the_gap_refuses_when_only_the_original_or_a_far_copy_is_locked() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let far = shifted(&mut doc, &[group], DVec3::new(0.0, 0.0, 300.0))[0];
        let pair = the_plane(&doc, &[group], 0);
        let undo = steps(&doc);

        for locked in [group, far] {
            with_flags(&mut doc, locked, false, true);
            let (nodes, undo) = (doc.map.nodes.clone(), steps(&doc));
            assert_eq!(doc.try_edit("Set Mirror Gap", |m, _| set_gap(m, &pair, 12.0)), Err(locked));
            assert!(doc.map.nodes == nodes && steps(&doc) == undo);
            with_flags(&mut doc, locked, false, false);
        }

        assert!(steps(&doc) > undo);
        assert_eq!(set(&mut doc, &pair, 12.0), 12.0);
        assert_eq!(gap(&doc.map, &pair), 12.0);
        assert!(doc.map.solid_bounds(copy).min.x > doc.map.solid_bounds(group).max.x);
    }

    #[test]
    fn mirror_again_moves_a_hidden_member_and_refuses_a_locked_one() {
        let slide = DVec3::new(0.0, -5.0, 12.0);
        for (hidden, locked) in [(true, false), (false, true)] {
            let mut doc = Document::new();
            let group = group_of(&mut doc, wedge(), "half");
            let copy = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
            moved(&mut doc, &[group], slide);
            with_flags(&mut doc, copy, hidden, locked);
            let (nodes, undo) = (doc.map.nodes.clone(), steps(&doc));
            let at = doc.map.solid_bounds(copy);
            let result = again_result(&mut doc, &[group]);
            if locked {
                assert_eq!(result, Err(AgainError::Locked(copy)));
                assert!(doc.map.nodes == nodes && steps(&doc) == undo, "nothing moves and no undo step is added");
                assert!(!drifted(&doc.map, &doc.selection).is_empty(), "the half is still slid");
            } else {
                assert_eq!(result.map(|r| r.0), Ok(1));
                assert_eq!(doc.map.solid_bounds(copy).min - at.min, slide, "a hidden copy moves with its half");
                assert_eq!(steps(&doc), undo + 1);
            }
        }
    }

    #[test]
    fn mirror_again_checks_only_the_members_it_moves() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 0, PlaneAt::Max).0[0];
        let far = shifted(&mut doc, &[group], DVec3::new(0.0, 0.0, 300.0))[0];
        moved(&mut doc, &[group], DVec3::new(0.0, 7.0, 3.0));
        with_flags(&mut doc, far, false, true);
        assert_eq!(again(&mut doc, &[group]).unwrap().0, 1, "a locked copy placed elsewhere stays and does not block");
        assert!(doc.map.get(far).unwrap().locked);

        moved(&mut doc, &[group], DVec3::new(0.0, 7.0, 3.0));
        with_flags(&mut doc, group, false, true);
        assert_eq!(again(&mut doc, &[group]).unwrap().0, 1, "the half that stays may be locked");

        with_flags(&mut doc, group, false, false);
        with_flags(&mut doc, far, false, false);
        moved(&mut doc, &[copy], DVec3::new(0.0, 7.0, 3.0));
        with_flags(&mut doc, group, false, true);
        let (nodes, undo) = (doc.map.nodes.clone(), steps(&doc));
        assert_eq!(again_result(&mut doc, &[copy]), Err(AgainError::Locked(group)));
        assert!(doc.map.nodes == nodes && steps(&doc) == undo);
    }

    #[test]
    fn setting_the_gap_keeps_the_other_planes_of_quarters() {
        let mut doc = Document::new();
        let a = group_of(&mut doc, wedge(), "quarter");
        let b = mirror(&mut doc, &[a], 0, PlaneAt::Max).0[0];
        let [c, d]: [NodeId; 2] = mirror(&mut doc, &[a, b], 2, PlaneAt::Min).0.try_into().unwrap();
        let pair = the_plane(&doc, &[a], 0);
        assert_eq!(set(&mut doc, &pair, 10.0), 10.0);

        select(&mut doc, &[a]);
        assert_eq!(
            selection_planes(&doc.map, &doc.selection),
            [
                MirrorPlane { plane: plane(0, 40.0), group: a, partner: b, span: Aabb::new(DVec3::new(3.0, 2.0, 4.0), DVec3::new(77.0, 34.0, 20.0)) },
                MirrorPlane { plane: plane(2, 4.0), group: a, partner: c, span: Aabb::new(DVec3::new(3.0, 2.0, -12.0), DVec3::new(35.0, 34.0, 20.0)) },
            ]
        );
        assert_eq!(partners(&doc.map, d), [(b, plane(2, 4.0)), (c, plane(0, 40.0))]);
        assert_eq!(gap(&doc.map, &the_plane(&doc, &[d], 0)), 10.0);
    }

    #[test]
    fn mirror_linked_with_a_gap_leaves_the_original_in_place() {
        let cases = [
            (PlaneAt::Max, 0.0, 40.0, 0.0),
            (PlaneAt::Max, 16.0, 48.0, 16.0),
            (PlaneAt::Min, 16.0, 0.0, 16.0),
            (PlaneAt::Center, 16.0, 32.0, -16.0),
            (PlaneAt::Center, -8.0, 24.0, -32.0),
            (PlaneAt::World, 16.0, -8.0, 32.0),
            (PlaneAt::Custom, 16.0, 108.25, 136.5),
            (PlaneAt::Max, -10.0, 35.0, -10.0),
            (PlaneAt::Max, -32.0, 24.0 + COVER_SLACK / 2.0, -32.0),
            (PlaneAt::Max, -40.0, 24.0 + COVER_SLACK / 2.0, -32.0),
            (PlaneAt::Max, 1e300, 40.0 + MAX_GAP / 2.0, MAX_GAP),
        ];
        for (at, by, want, measured) in cases {
            let mut doc = Document::new();
            let group = group_of(&mut doc, wedge(), "half");
            select(&mut doc, &[group]);
            let choice = PlaneChoice { at, custom: 100.25 };
            let (copies, found) = doc.edit("Mirror Linked", |m, s| mirror_linked(m, s, 0, &choice, by)).unwrap();
            assert_eq!(found, plane(0, want), "{at:?} {by}");
            assert_bounds(doc.map.solid_bounds(group), WEDGE);
            assert_copy_maps(&doc.map, group, copies[0], &found.matrix());
            let pair = MirrorPlane { plane: found, group, partner: copies[0], span: Aabb::EMPTY };
            assert_eq!(gap(&doc.map, &pair), measured, "{at:?} {by}");
        }
    }

    #[test]
    fn quarters_become_one_brush_and_one_closed_mesh() {
        let mut doc = Document::new();
        let a = furnished(&mut doc);
        let b = mirror(&mut doc, &[a], 0, PlaneAt::Min).0[0];
        let [c, d]: [NodeId; 2] = mirror(&mut doc, &[a, b], 2, PlaneAt::Max).0.try_into().unwrap();
        let leaves: Vec<[NodeId; 3]> = [a, b, c, d].iter().map(|g| children(&doc.map, *g).try_into().unwrap()).collect();
        let [brush, mesh, _] = leaves[0];

        let applied = apply(&mut doc, &[leaves[3][1]], MIRROR_TOLERANCE).unwrap();
        let mut replaced = Replaced::new();
        for (g, [x, y, _]) in [b, c, d].into_iter().zip(&leaves[1..]) {
            replaced.extend([(g, vec![a]), (*x, vec![brush]), (*y, vec![mesh])]);
        }

        assert_eq!(applied, Applied { groups: vec![a], joined: 3, welded: 1, replaced });
        assert_unlinked(&doc.map, a, "half");
        assert_eq!((of_kind(&doc.map, a, "brush"), of_kind(&doc.map, a, "mesh"), of_kind(&doc.map, a, "entity").len()), (vec![brush], vec![mesh], 4));

        let joined = doc.map.brush(brush).unwrap();
        joined.validate().unwrap();
        assert_eq!(joined.faces.len(), 9);
        let end = |x: f64| [DVec3::new(x, 2.0, 4.0), DVec3::new(x, 2.0, 36.0), DVec3::new(x, 14.0, 4.0), DVec3::new(x, 18.0, 20.0), DVec3::new(x, 14.0, 36.0)];
        let ridge = [DVec3::new(8.0, 30.0, 4.0), DVec3::new(8.0, 34.0, 20.0), DVec3::new(8.0, 30.0, 36.0)];
        assert_points(&joined.vertices, &[end(40.0).as_slice(), &end(-24.0), &ridge].concat());

        let welded = doc.map.mesh(mesh).unwrap();
        assert!(welded.is_closed());
        assert_eq!((welded.vertices.len(), welded.faces.len()), (18, 16));
        assert_bounds(welded.bounds(), Aabb::new(DVec3::new(-8.0, 40.0, 6.0), DVec3::new(24.0, 59.5, 34.0)));
        assert_no_face_on(welded, plane(0, 8.0));
        assert_no_face_on(welded, plane(2, 20.0));
    }

    fn moved(doc: &mut Document, ids: &[NodeId], offset: DVec3) {
        select(doc, ids);
        doc.edit("move", |m, s| ops::translate_selection(m, s, offset, EditOptions::default()));
    }

    fn again(doc: &mut Document, ids: &[NodeId]) -> Option<(usize, f64, AxisPlane)> {
        again_result(doc, ids).ok()
    }

    fn again_result(doc: &mut Document, ids: &[NodeId]) -> Result<(usize, f64, AxisPlane), AgainError> {
        select(doc, ids);
        doc.try_edit("Mirror Again", |m, s| mirror_again(m, s))
    }

    fn steps(doc: &Document) -> usize {
        doc.history.undo_labels().count()
    }

    fn with_flags(doc: &mut Document, id: NodeId, hidden: bool, locked: bool) {
        doc.edit("flags", |m, _| {
            ops::set_hidden(m, &[id], hidden);
            ops::set_locked(m, &[id], locked);
        });
    }

    #[test]
    fn mirror_again_brings_the_partner_back_across_from_the_selected_half() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let (copies, found) = mirror(&mut doc, &[group], 0, PlaneAt::Max);
        let copy = copies[0];
        let slide = DVec3::new(0.0, -5.0, 12.0);
        moved(&mut doc, &[group], slide);
        let (half, partner) = (transform(&doc.map, group), transform(&doc.map, copy));
        select(&mut doc, &[group]);
        assert_eq!(selection_planes(&doc.map, &doc.selection), [], "a slid half has no plane drawn");
        let drifts = drifted(&doc.map, &doc.selection);
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert_eq!((drifts[0].group, drifts[0].partner, drifts[0].plane.axis), (group, copy, 0));
        assert!((drifts[0].plane.at - found.at).abs() < 1e-9 && drifts[0].slide.abs_diff_eq(-slide, 1e-9), "{drifts:?}");

        let (count, distance, again_plane) = again(&mut doc, &[group]).unwrap();
        assert_eq!(count, 1);
        assert!((distance - slide.length()).abs() < 1e-9, "{distance}");
        assert_eq!(again_plane, drifts[0].plane);
        assert_eq!(transform(&doc.map, group), half, "the selected half stays");
        assert!(transform(&doc.map, copy).abs_diff_eq(DMat4::from_translation(slide) * partner, 1e-9));
        assert_eq!(partners(&doc.map, group).len(), 1);
        assert!((partners(&doc.map, group)[0].1.at - found.at).abs() < 1e-9 && partners(&doc.map, group)[0].0 == copy);
        assert_copy_maps(&doc.map, group, copy, &found.matrix());
        assert_eq!(drifted(&doc.map, &doc.selection), []);
        assert_eq!(doc.undo().as_deref(), Some("Mirror Again"));
        assert!(transform(&doc.map, copy).abs_diff_eq(partner, 1e-12), "one undo step puts the partner back");

        moved(&mut doc, &[group], -slide);
        moved(&mut doc, &[copy], slide);
        let child = doc.map.get(group).unwrap().children[0];
        let (copy_before, half_before) = (transform(&doc.map, copy), transform(&doc.map, group));
        assert_eq!(again(&mut doc, &[copy]).unwrap().0, 1);
        assert_eq!(transform(&doc.map, copy), copy_before, "the selected partner stays");
        assert!(transform(&doc.map, group).abs_diff_eq(DMat4::from_translation(slide) * half_before, 1e-9));
        doc.undo();
        assert_eq!(again(&mut doc, &[child]).unwrap().0, 1);
        assert_eq!(transform(&doc.map, group), half_before, "the half around a selected brush stays");
        doc.undo();
        assert_eq!(again(&mut doc, &[copy, group]).unwrap().0, 1);
        assert_eq!(transform(&doc.map, group), half_before, "with both selected the lower id stays");
    }

    #[test]
    fn a_slide_within_the_slack_is_no_drift() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let copy = mirror(&mut doc, &[group], 2, PlaneAt::Min).0[0];
        moved(&mut doc, &[copy], DVec3::new(0.9e-3, 0.0, 0.0));
        select(&mut doc, &[group]);
        assert_eq!(drifted(&doc.map, &doc.selection), []);
        assert_eq!(selection_planes(&doc.map, &doc.selection).len(), 1);
        moved(&mut doc, &[copy], DVec3::new(0.2e-3, 0.0, 0.0));
        select(&mut doc, &[group]);
        assert_eq!(drifted(&doc.map, &doc.selection).len(), 1);
        assert_eq!(selection_planes(&doc.map, &doc.selection), []);
    }

    #[test]
    fn mirror_again_moves_a_facade_row_and_leaves_an_offset_copy() {
        let mut doc = Document::new();
        let [[a, b], [c, d], [e, f]] = facade(&mut doc);
        let offset = shifted(&mut doc, &[a], DVec3::new(0.0, 200.0, 0.0))[0];
        let row = [b, c, d, e, f];
        let before = row.map(|id| transform(&doc.map, id));
        let (spare, half) = (transform(&doc.map, offset), transform(&doc.map, a));
        moved(&mut doc, &[a], DVec3::new(0.0, 0.0, -12.0));
        let (count, distance, plane_found) = again(&mut doc, &[a]).unwrap();
        assert_eq!((count, distance), (5, 12.0));
        assert!((plane_found.at - 40.0).abs() < 1e-9 && plane_found.axis == 0, "the nearest partner's plane, {plane_found:?}");
        for (id, was) in row.iter().zip(before) {
            assert!(transform(&doc.map, *id).abs_diff_eq(DMat4::from_translation(DVec3::new(0.0, 0.0, -12.0)) * was, 1e-9), "{id}");
        }

        assert_eq!(transform(&doc.map, offset), spare, "a copy placed elsewhere stays");
        assert!(transform(&doc.map, a).abs_diff_eq(DMat4::from_translation(DVec3::new(0.0, 0.0, -12.0)) * half, 1e-12));
        assert_eq!(partners(&doc.map, a).len(), 3);
    }

    #[test]
    fn mirror_again_with_nothing_drifted_changes_nothing() {
        let mut doc = Document::new();
        let group = group_of(&mut doc, wedge(), "half");
        let plain = group_of(&mut doc, wedge(), "plain");
        assert_eq!(again(&mut doc, &[]), None);
        assert_eq!(again(&mut doc, &[group]), None, "an unlinked group");
        let copy = mirror(&mut doc, &[group], 1, PlaneAt::Center).0[0];
        moved(&mut doc, &[copy], DVec3::new(0.0, 24.0, 0.0));
        let before = doc.map.nodes.clone();
        let undo: Vec<String> = doc.history.undo_labels().map(String::from).collect();
        assert_eq!(again(&mut doc, &[group]), None, "a move along the axis moves the plane");
        assert_eq!(again(&mut doc, &[copy]), None);
        assert_eq!(again(&mut doc, &[plain]), None);
        assert!(doc.map.nodes == before && doc.history.undo_labels().eq(undo.iter().map(String::as_str)), "a refused Mirror Again leaves no undo step");
        let twin = shifted(&mut doc, &[group], DVec3::new(0.0, 0.0, 64.0))[0];
        let before_twin = doc.map.nodes.clone();
        assert_eq!(again(&mut doc, &[twin]), None, "a copy beside a pair that still mirrors has nothing to fix");
        assert!(doc.map.nodes == before_twin);
    }
}
