//! A TrenchDot terrain drawn with the clipmap rings: its heights and paint live in two textures and the rings snap in
//! the terrain's own cells, so the near ground splits its cells like the chunk meshes do.

use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec2, I64Vec2, Mat4, Vec3};

use super::layout;
use crate::wgpu;

/// Bytes per vertex: an R32Float height and an Rg32Uint paint texel.
const VERTEX_BYTES: u64 = 12;

/// Whether a terrain of `resolution` vertices can be held: at least one cell, and no side longer than `max_side`, the
/// GPU's largest 2D texture.
pub fn fits(resolution: [u32; 2], max_side: u32) -> bool {
    resolution.iter().all(|side| (2..=max_side).contains(side))
}

/// Where and how the terrain draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainClipParams {
    /// World position of the first vertex. Heights are added to its y.
    pub origin: Vec3,
    /// Map units between vertices.
    pub cell: f32,
    pub selected: bool,
    /// The lightmap chart, uv2 is each row dotted with (world, 1). None before a bake.
    pub chart: Option<[[f32; 4]; 2]>,
}

impl Default for TerrainClipParams {
    fn default() -> Self {
        TerrainClipParams { origin: Vec3::ZERO, cell: 32.0, selected: false, chart: None }
    }
}

/// Writes counted since the clipmap was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uploads {
    pub whole: u64,
    pub partial: u64,
    /// Vertices the partial writes covered.
    pub partial_cells: u64,
}

/// The native clipmap shader's uniform, see terrain_clip.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct TerrainClipUniform {
    /// xyz: world position of the first vertex, w: map units per cell
    origin: [f32; 4],
    /// xy: cells along x and z, z: 1 when selected, w: 1 with a lightmap chart
    grid: [u32; 4],
    chart: [[f32; 4]; 2],
}

impl TerrainClipUniform {
    fn new(cells: [u32; 2], p: &TerrainClipParams) -> Self {
        TerrainClipUniform {
            origin: p.origin.extend(p.cell).to_array(),
            grid: [cells[0], cells[1], p.selected as u32, p.chart.is_some() as u32],
            chart: p.chart.unwrap_or([[0.0; 4]; 2]),
        }
    }
}

pub(crate) fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding: u32, visibility, sample_type| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain clipmap"),
        entries: &[
            texture(0, wgpu::ShaderStages::VERTEX, wgpu::TextureSampleType::Float { filterable: false }),
            texture(1, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Uint),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    })
}

/// One native terrain on the GPU. Heights are map units above the origin's y, row by row along z like
/// `Terrain::heights`. A paint texel holds the four splat bytes of its vertex in x, layer 0 in the low byte, and in
/// bit 0 of y whether the cell starting at that vertex is a hole.
pub struct TerrainClipmap {
    resolution: [u32; 2],
    heights: wgpu::Texture,
    paint: wgpu::Texture,
    /// Lowest and highest height written, for culling. Partial writes only widen it.
    height_range: [f32; 2],
    params: TerrainClipParams,
    material: String,
    uploads: Uploads,
    buffer: wgpu::Buffer,
    pub(crate) bind_group: wgpu::BindGroup,
    triangles: AtomicU64,
}

impl TerrainClipmap {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, resolution: [u32; 2]) -> Result<Self, String> {
        let max_side = device.limits().max_texture_dimension_2d;
        if !fits(resolution, max_side) {
            let [w, h] = resolution;
            return Err(format!("a terrain of {w} by {h} vertices does not fit textures of at most {max_side} texels"));
        }

        let texture = |label: &str, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: resolution[0], height: resolution[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let heights = texture("terrain clipmap heights", wgpu::TextureFormat::R32Float);
        let paint = texture("terrain clipmap paint", wgpu::TextureFormat::Rg32Uint);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain clipmap"),
            size: std::mem::size_of::<TerrainClipUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let [heights_view, paint_view] = [&heights, &paint].map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain clipmap"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&heights_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&paint_view) },
                wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
            ],
        });
        let clip = TerrainClipmap {
            resolution,
            heights,
            paint,
            height_range: [0.0; 2],
            params: TerrainClipParams::default(),
            material: String::new(),
            uploads: Uploads::default(),
            buffer,
            bind_group,
            triangles: AtomicU64::new(0),
        };
        queue.write_buffer(&clip.buffer, 0, bytemuck::bytes_of(&clip.uniform()));
        Ok(clip)
    }

    pub fn resolution(&self) -> [u32; 2] {
        self.resolution
    }

    pub fn cells(&self) -> [u32; 2] {
        self.resolution.map(|side| side - 1)
    }

    pub fn params(&self) -> &TerrainClipParams {
        &self.params
    }

    /// The terrain material draw key, from `Renderer::prepare_terrain_material`.
    pub fn material(&self) -> &str {
        &self.material
    }

    pub fn uploads(&self) -> Uploads {
        self.uploads
    }

    /// Triangles the rings drew in the last main pass that showed this terrain, after culling.
    pub fn triangles(&self) -> u64 {
        self.triangles.load(Ordering::Relaxed)
    }

    pub(crate) fn drew(&self, triangles: usize) {
        self.triangles.store(triangles as u64, Ordering::Relaxed);
    }

    pub fn gpu_bytes(&self) -> u64 {
        VERTEX_BYTES * self.resolution[0] as u64 * self.resolution[1] as u64
    }

    fn uniform(&self) -> TerrainClipUniform {
        TerrainClipUniform::new(self.cells(), &self.params)
    }

    pub fn set_params(&mut self, queue: &wgpu::Queue, params: &TerrainClipParams) {
        self.params = *params;
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniform()));
    }

    pub fn set_material(&mut self, key: &str) {
        key.clone_into(&mut self.material);
    }

    /// Writes every vertex.
    pub fn write_whole(&mut self, queue: &wgpu::Queue, heights: &[f32], paint: &[[u32; 2]]) -> Result<(), String> {
        self.check([0, 0], self.resolution, heights, paint)?;
        self.write(queue, [0, 0], self.resolution, heights, paint);
        self.height_range = range(heights);
        self.uploads.whole += 1;
        Ok(())
    }

    /// Writes a rectangle of vertices, `origin` and `size` in vertices, the slices row by row.
    pub fn write_rect(&mut self, queue: &wgpu::Queue, origin: [u32; 2], size: [u32; 2], heights: &[f32], paint: &[[u32; 2]]) -> Result<(), String> {
        self.check(origin, size, heights, paint)?;
        if heights.is_empty() {
            return Ok(());
        }

        self.write(queue, origin, size, heights, paint);
        let [lo, hi] = range(heights);
        self.height_range = [self.height_range[0].min(lo), self.height_range[1].max(hi)];
        self.uploads.partial += 1;
        self.uploads.partial_cells += heights.len() as u64;
        Ok(())
    }

    fn check(&self, origin: [u32; 2], size: [u32; 2], heights: &[f32], paint: &[[u32; 2]]) -> Result<(), String> {
        if (0..2).any(|i| origin[i].checked_add(size[i]).is_none_or(|end| end > self.resolution[i])) {
            return Err(format!("rectangle {origin:?} of {size:?} leaves a terrain of {:?} vertices", self.resolution));
        }

        let n = size[0] as usize * size[1] as usize;
        if heights.len() != n || paint.len() != n {
            return Err(format!("{} heights and {} paint texels for {n} vertices", heights.len(), paint.len()));
        }

        Ok(())
    }

    fn write(&self, queue: &wgpu::Queue, origin: [u32; 2], size: [u32; 2], heights: &[f32], paint: &[[u32; 2]]) {
        let at = wgpu::Origin3d { x: origin[0], y: origin[1], z: 0 };
        super::write(queue, &self.heights, at, size, bytemuck::cast_slice(heights));
        super::write(queue, &self.paint, at, size, bytemuck::cast_slice(paint));
    }

    /// The parts a view at `eye` draws over the terrain. The rings snap in the terrain's cells, so a part's origin is
    /// in cells from the first vertex.
    pub fn instances(&self, eye: Vec3, view_proj: Mat4) -> layout::Instances {
        let p = &self.params;
        let cell = p.cell as f64;
        let local = (DVec2::new(eye.x as f64, eye.z as f64) - DVec2::new(p.origin.x as f64, p.origin.z as f64)) / cell;
        let [x, z] = self.cells().map(i64::from);
        let cull = layout::Cull {
            frustum: Some(layout::Frustum::new(view_proj * Mat4::from_translation(p.origin))),
            cell,
            heights: self.height_range.map(f64::from),
            area: Some([I64Vec2::ZERO, I64Vec2::new(x, z)]),
        };
        layout::Instances::new(local, &cull)
    }
}

fn range(heights: &[f32]) -> [f32; 2] {
    heights.iter().fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], &h| [lo.min(h), hi.max(h)])
}

#[cfg(test)]
mod tests {
    use glam::camera::rh::proj::directx::perspective_infinite_reverse;
    use glam::camera::rh::view::look_to_mat4;

    use glam::{DVec3, Vec2};
    use td_geom::Terrain;

    use super::*;
    use crate::clipmap::tests::{device, render_parts, render_shadow_depths, top_down};

    const ORIGIN: Vec3 = Vec3::new(-1000.5, 37.0, 2210.0);
    const CELL: f32 = 24.0;

    fn terrain(resolution: [u32; 2], origin: Vec3) -> (wgpu::Device, wgpu::Queue, TerrainClipmap) {
        let (device, queue) = device(true);
        let mut clip = TerrainClipmap::new(&device, &queue, &bind_group_layout(&device), resolution).unwrap();
        clip.set_params(&queue, &TerrainClipParams { origin, cell: CELL, ..Default::default() });
        (device, queue, clip)
    }

    /// Rises along x, falls along z.
    fn slanted([w, h]: [u32; 2]) -> Vec<f32> {
        (0..h).flat_map(|j| (0..w).map(move |i| i as f32 * 3.5 - j as f32 * 11.0 + 40.0)).collect()
    }

    fn at_cells(cells: [f32; 2], up: f32) -> Vec3 {
        ORIGIN + Vec3::new(cells[0] * CELL, up, cells[1] * CELL)
    }

    fn look(eye: Vec3, dir: Vec3, fov: f32) -> Mat4 {
        let up = if dir.x == 0.0 && dir.z == 0.0 { Vec3::Z } else { Vec3::Y };
        perspective_infinite_reverse(fov, 1.5, 1.0) * look_to_mat4(eye, dir.normalize(), up)
    }

    fn open(eye: Vec3) -> Mat4 {
        top_down(eye, [1.0e7, 1.0e7])
    }

    fn overlaps(part: &layout::Part, cells: I64Vec2) -> bool {
        let [min, max] = part.cells();
        min.max(I64Vec2::ZERO).cmplt(max.min(cells)).all()
    }

    #[test]
    fn fits_takes_sides_from_two_vertices_to_the_largest_texture() {
        let cases = [
            ([2049, 2049], 8192, true),
            ([2, 2], 8192, true),
            ([8192, 3], 8192, true),
            ([3, 8192], 8192, true),
            ([8193, 3], 8192, false),
            ([3, 8193], 8192, false),
            ([2048, 2048], 2048, true),
            ([2049, 2048], 2048, false),
            ([1, 5], 8192, false),
            ([5, 1], 8192, false),
            ([0, 0], 8192, false),
        ];
        for (resolution, max_side, want) in cases {
            assert_eq!(fits(resolution, max_side), want, "{resolution:?} under {max_side}");
        }

        let (device, queue) = device(true);
        let layout = bind_group_layout(&device);
        assert_eq!(device.limits().max_texture_dimension_2d, 8192);
        for resolution in [[8193, 3], [3, 8193], [1, 2]] {
            assert!(TerrainClipmap::new(&device, &queue, &layout, resolution).is_err(), "{resolution:?}");
        }

        let clip = TerrainClipmap::new(&device, &queue, &layout, [37, 21]).unwrap();
        assert_eq!((clip.resolution(), clip.cells(), clip.gpu_bytes()), ([37, 21], [36, 20], 12 * 37 * 21));
        assert_eq!((clip.uploads(), clip.height_range, clip.material()), (Uploads::default(), [0.0; 2], ""));
    }

    #[test]
    fn writes_outside_the_terrain_are_refused_and_only_done_ones_count() {
        let (_device, queue, mut clip) = terrain([37, 21], ORIGIN);
        let n = 37 * 21;
        let heights = slanted([37, 21]);
        let paint = vec![[0x01_02_03_f9u32, 1]; n];
        assert!(clip.write_whole(&queue, &heights[1..], &paint).is_err(), "one height short");
        assert!(clip.write_whole(&queue, &heights, &paint[..n - 1]).is_err(), "one paint texel short");
        assert_eq!((clip.uploads(), clip.height_range), (Uploads::default(), [0.0; 2]), "refused writes change nothing");

        assert_eq!(clip.write_whole(&queue, &heights, &paint), Ok(()));
        assert_eq!(clip.height_range, [-180.0, 166.0]);
        let mut rect = |origin, size: [u32; 2], short: usize, h: f32| {
            let m = (size[0] * size[1]) as usize;
            clip.write_rect(&queue, origin, size, &vec![h; m][short..], &paint[..m])
        };
        assert_eq!(rect([30, 15], [7, 6], 0, 10.0), Ok(()), "a rectangle ending on the far corner");
        assert!(rect([31, 15], [7, 6], 0, 10.0).is_err(), "one vertex past x");
        assert!(rect([30, 16], [7, 6], 0, 10.0).is_err(), "one vertex past z");
        assert!(rect([u32::MAX, 0], [2, 1], 0, 10.0).is_err(), "an origin that overflows");
        assert!(rect([0, 0], [4, 3], 1, 10.0).is_err(), "one height short");
        assert_eq!(rect([3, 4], [1, 1], 0, 900.0), Ok(()), "one vertex");
        assert_eq!(rect([5, 5], [0, 0], 0, -900.0), Ok(()), "an empty rectangle writes nothing");
        assert_eq!(rect([37, 21], [0, 0], 0, -900.0), Ok(()), "an empty rectangle on the far corner");
        assert_eq!(clip.uploads(), Uploads { whole: 1, partial: 2, partial_cells: 43 });
        assert_eq!(clip.height_range, [-180.0, 900.0], "a partial write only widens the range");

        assert_eq!(clip.write_whole(&queue, &vec![-7.5; n], &paint), Ok(()));
        assert_eq!(clip.height_range, [-7.5, -7.5], "a whole write sets it");
        assert_eq!(clip.write_rect(&queue, [35, 0], [2, 1], &[-9.0, 4.0], &paint[..2]), Ok(()));
        assert_eq!(clip.height_range, [-9.0, 4.0]);
        assert_eq!(clip.uploads(), Uploads { whole: 2, partial: 3, partial_cells: 45 });
    }

    #[test]
    fn params_and_material_reach_the_uniform() {
        let (_device, queue, mut clip) = terrain([37, 21], ORIGIN);
        assert_eq!(clip.uniform(), TerrainClipUniform { origin: [-1000.5, 37.0, 2210.0, 24.0], grid: [36, 20, 0, 0], chart: [[0.0; 4]; 2] });

        let rows = [[0.25, -0.5, 0.125, 3.0], [-0.0625, 0.75, 1.5, -2.0]];
        let params = TerrainClipParams { origin: Vec3::new(64.0, -12.5, -333.25), cell: 40.0, selected: true, chart: Some(rows) };
        clip.set_params(&queue, &params);
        assert_eq!(*clip.params(), params);
        assert_eq!(clip.uniform(), TerrainClipUniform { origin: [64.0, -12.5, -333.25, 40.0], grid: [36, 20, 1, 1], chart: rows });

        clip.set_params(&queue, &TerrainClipParams { selected: false, chart: None, ..params });
        assert_eq!(clip.uniform(), TerrainClipUniform { origin: [64.0, -12.5, -333.25, 40.0], grid: [36, 20, 0, 0], chart: [[0.0; 4]; 2] });

        clip.set_material("rock|grass");
        clip.set_material("sand");
        assert_eq!(clip.material(), "sand");
    }

    #[test]
    fn instances_cover_the_terrain_and_nothing_past_it() {
        for (resolution, local, levels) in [([2, 2], [0.75, -3.25], 1), ([129, 65], [40.25, 13.5], 1), ([1025, 513], [-301.5, 700.75], 3)] {
            let cells = I64Vec2::new(resolution[0] as i64 - 1, resolution[1] as i64 - 1);
            let (_device, _queue, clip) = terrain(resolution, ORIGIN);
            let eye = at_cells(local, 900.0);
            let got = clip.instances(eye, open(eye));
            let target = DVec2::new(local[0] as f64, local[1] as f64);
            let mut want: Vec<layout::Part> = layout::ring(target).into_iter().filter(|p| overlaps(p, cells)).collect();
            want.sort_by_key(|p| p.kind);
            assert_eq!(got.parts, want, "{resolution:?}");
            let mut drawn: Vec<u32> = got.parts.iter().map(|p| p.level).collect();
            drawn.sort();
            drawn.dedup();
            assert_eq!(drawn.len(), levels, "{resolution:?} draws levels {drawn:?}");
        }

        let (_device, _queue, one) = terrain([2, 2], ORIGIN);
        let eye = at_cells([0.75, -3.25], 900.0);
        let parts = one.instances(eye, open(eye)).parts;
        assert_eq!(parts.len(), 1, "one cell lies in one part");
        let [min, max] = parts[0].cells();
        assert!(min.cmple(I64Vec2::ZERO).all() && max.cmpge(I64Vec2::ONE).all(), "{parts:?}");

        let far = at_cells([1.0e5, 0.0], 900.0);
        assert!(one.instances(far, open(far)).parts.is_empty(), "past the outer ring");
    }

    #[test]
    fn instances_move_with_the_terrain() {
        let resolution = [257, 129];
        let eye = Vec3::new(-700.5, 900.0, 1500.25);
        let dir = Vec3::new(1.0, -0.2, 0.0);
        let instances = |offset: Vec3| {
            let (_device, queue, mut clip) = terrain(resolution, ORIGIN + offset);
            let heights = slanted(resolution);
            clip.write_whole(&queue, &heights, &vec![[0, 0]; heights.len()]).unwrap();
            let e = eye + offset;
            (clip.instances(e, look(e, dir, 0.3)), clip.instances(e, open(e)))
        };
        let (here, all) = instances(Vec3::ZERO);
        let (there, _) = instances(Vec3::new(-3000.0, 512.0, 6000.0));
        assert_eq!(here, there);
        assert!(!here.parts.is_empty() && here.parts.len() < all.parts.len(), "{} of {}", here.parts.len(), all.parts.len());
    }

    #[test]
    fn nothing_draws_behind_the_camera() {
        let (_device, queue, mut clip) = terrain([129, 129], ORIGIN);
        clip.write_whole(&queue, &slanted([129, 129]), &vec![[0, 0]; 129 * 129]).unwrap();
        let eye = at_cells([40.5, 70.25], -2000.0);
        assert!(clip.instances(eye, look(eye, Vec3::NEG_Y, 1.0)).parts.is_empty(), "under the lowest vertex looking down");
        assert!(!clip.instances(eye, look(eye, Vec3::Y, 1.0)).parts.is_empty(), "looking up");
    }

    #[test]
    fn written_heights_widen_the_culling() {
        let (_device, queue, mut clip) = terrain([129, 129], ORIGIN);
        let eye = at_cells([64.0, -4.0], 5000.0);
        let view = look(eye, Vec3::Z, 0.2);
        let paint = vec![[0, 0]; 129 * 129];
        assert!(clip.instances(eye, view).parts.is_empty(), "flat ground far under a level view");

        clip.write_rect(&queue, [64, 70], [1, 1], &[5000.0], &paint[..1]).unwrap();
        assert!(!clip.instances(eye, view).parts.is_empty(), "a raised vertex");

        clip.write_whole(&queue, &vec![0.0; 129 * 129], &paint).unwrap();
        assert!(clip.instances(eye, view).parts.is_empty(), "flattened again");
    }

    #[test]
    fn level_0_parts_start_on_even_cells() {
        let (_device, _queue, clip) = terrain([513, 257], ORIGIN);
        for local in [[0.5, 0.5], [17.25, 3.75], [-2.5, 101.0], [255.99, 128.01], [1.0, -1.0]] {
            let eye = at_cells(local, 300.0);
            let level_0: Vec<I64Vec2> = clip.instances(eye, open(eye)).parts.iter().filter(|p| p.level == 0).map(|p| p.origin).collect();
            assert!(!level_0.is_empty(), "{local:?}");
            assert!(level_0.iter().all(|o| o.rem_euclid(I64Vec2::splat(2)) == I64Vec2::ZERO), "{local:?}: {level_0:?}");
        }
    }

    /// A slanted and bumpy terrain at `ORIGIN` with four painted layers, a vertex whose splat sums to 0, a hole inside
    /// and one in the last cell.
    fn painted() -> Terrain {
        let [w, d] = [37u32, 21];
        let mut t = Terrain::new(ORIGIN.as_dvec3(), [w, d], CELL as f64, "stone");
        t.heights = (0..d).flat_map(|j| (0..w).map(move |i| i as f32 * 3.5 - j as f32 * 11.0 + 40.0 + ((i * 7 + j * 3) % 5) as f32 * 9.0)).collect();
        t.splat = (0..w * d).flat_map(|k| [(k * 37 % 256) as u8, (k * 11 % 200) as u8, (k % 3 * 60) as u8, 30]).collect();
        let zero = t.index(12, 7) * 4;
        t.splat[zero..zero + 4].fill(0);
        t.holes = vec![0; 36 * 20];
        t.holes[5 * 36 + 10] = 1;
        t.holes[19 * 36 + 35] = 1;
        t
    }

    fn pack(t: &Terrain) -> Vec<[u32; 2]> {
        let [w, d] = t.resolution;
        let [cw, cd] = t.cells();
        (0..d)
            .flat_map(|j| (0..w).map(move |i| (i, j)))
            .map(|(i, j)| {
                let k = t.index(i, j) * 4;
                let splat = t.splat.get(k..k + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                [splat, (i < cw && j < cd && t.is_hole(i, j)) as u32]
            })
            .collect()
    }

    const SIZE: [u32; 2] = [192, 108];

    /// Draws `t` from straight above with `fragment`, an `fs_test` entry added to the shader, and returns the pixels
    /// with the world x and z under each pixel's center.
    fn draw_from_above(t: &Terrain, params: TerrainClipParams, fragment: &str) -> Vec<([f32; 4], DVec2)> {
        let (device, queue) = device(false);
        let layout = bind_group_layout(&device);
        let mut clip = TerrainClipmap::new(&device, &queue, &layout, t.resolution).unwrap();
        clip.write_whole(&queue, &t.heights, &pack(t)).unwrap();
        clip.set_params(&queue, &params);
        let size = t.size().as_vec2();
        let eye = ORIGIN + Vec3::new(size.x / 2.0, 3000.0, size.y / 2.0);
        let view_proj = top_down(eye, [480.0, 270.0]);
        let source = format!("{}\n{fragment}", crate::terrain_clip_shader());
        let groups = [None, None, Some((&layout, &clip.bind_group))];
        let pixels = render_parts(&device, &queue, &source, &groups, &clip.instances(eye, view_proj), eye, view_proj, SIZE);
        let [w, h] = SIZE;
        let inverse = view_proj.inverse();
        let under = (0..w * h).map(|k| {
            let ndc = Vec3::new(((k % w) as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - ((k / w) as f32 + 0.5) / h as f32 * 2.0, 0.5);
            let at = inverse.project_point3(ndc);
            DVec2::new(at.x as f64, at.z as f64)
        });
        pixels.into_iter().zip(under).collect()
    }

    fn local(t: &Terrain, xz: DVec2) -> DVec2 {
        (xz - DVec2::new(t.origin.x, t.origin.z)) / t.cell_size
    }

    /// Whether `xz` is inside the terrain and whether it is over a hole, None within a small margin of a cell line where
    /// the rasterizer may go either way.
    fn spot(t: &Terrain, xz: DVec2) -> Option<(bool, bool)> {
        let local = local(t, xz);
        let f = local - local.floor();
        if f.min_element() < 0.03 || f.max_element() > 0.97 {
            return None;
        }

        let [cw, cd] = t.cells();
        let inside = local.x > 0.0 && local.y > 0.0 && local.x < cw as f64 && local.y < cd as f64;
        Some((inside, inside && t.is_hole(local.x as u32, local.y as u32)))
    }

    fn on_ground(t: &Terrain, xz: DVec2) -> bool {
        spot(t, xz) == Some((true, false))
    }

    /// The vertex weights of the triangle under `xz` mixed by where `xz` sits in it, what the chunk meshes interpolate.
    fn chunk_weights(t: &Terrain, xz: DVec2) -> [f64; 4] {
        let local = local(t, xz);
        for tri in Terrain::cell_triangles(local.x as u32, local.y as u32) {
            let [a, b, c] = tri.map(|(i, j)| DVec2::new(i as f64, j as f64));
            let det = (b - a).perp_dot(c - a);
            let u = (local - a).perp_dot(c - a) / det;
            let v = (b - a).perp_dot(local - a) / det;
            if u >= -1e-9 && v >= -1e-9 && u + v <= 1.0 + 1e-9 {
                let [wa, wb, wc] = tri.map(|(i, j)| t.weights(i, j).map(f64::from));
                return std::array::from_fn(|k| wa[k] * (1.0 - u - v) + wb[k] * u + wc[k] * v);
            }
        }

        unreachable!("{local} is in neither triangle of its cell")
    }

    /// The level 0 rings follow the terrain's own triangles, so every pixel lands on `Terrain::height_at`, and the
    /// edge and both holes are cut.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_draws_the_heights_and_holes_of_the_terrain() {
        let t = painted();
        let fragment =
            "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { if (dropped(local_cells(f.world))) { discard; } return vec4<f32>(f.world, 1.0); }";
        let pixels = draw_from_above(&t, TerrainClipParams { origin: ORIGIN, cell: CELL, ..Default::default() }, fragment);

        let [mut ground, mut past, mut holes] = [0, 0, 0];
        for (k, (p, xz)) in pixels.iter().enumerate() {
            let Some((inside, hole)) = spot(&t, *xz) else { continue };
            assert_eq!(p[3] == 1.0, inside && !hole, "pixel {k} at {xz}: {p:?}");
            if !inside {
                past += 1;
            } else if hole {
                holes += 1;
            } else {
                let want = DVec3::new(xz.x, t.height_at(xz.x, xz.y).unwrap(), xz.y);
                assert!(Vec3::from_slice(&p[..3]).as_dvec3().abs_diff_eq(want, 0.05), "pixel {k} drew {p:?}, want {want}");
                ground += 1;
            }
        }

        assert!(ground > 10000 && past > 2500 && holes > 25, "{ground} ground, {past} past the edge and {holes} hole pixels checked");
    }

    /// A thousandth of a cell past the edge still draws, so tiled terrains meet without a gap, a hundredth does not.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_keeps_a_sliver_past_the_edge() {
        let fragment = "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { let g = vec2<f32>(clip.grid.xy);             return vec4<f32>(f32(dropped(vec2<f32>(g.x + 5e-4, 1.5))), f32(dropped(vec2<f32>(1.5, -5e-4))),             f32(dropped(vec2<f32>(g.x + 0.01, 1.5))), f32(dropped(vec2<f32>(1.5, -0.01)))); }";
        let pixels = draw_from_above(&painted(), TerrainClipParams { origin: ORIGIN, cell: CELL, ..Default::default() }, fragment);
        let drawn: Vec<[f32; 4]> = pixels.into_iter().map(|(p, _)| p).filter(|p| p[3] == 1.0).collect();
        assert!(drawn.len() > 10000, "{} pixels drawn", drawn.len());
        assert!(drawn.iter().all(|p| *p == [0.0, 0.0, 1.0, 1.0]), "{:?}", drawn.iter().find(|p| **p != [0.0, 0.0, 1.0, 1.0]));
    }

    /// Paint at every pixel is the mix of the chunk meshes, a vertex with no splat counts as the first layer, and uv2
    /// is the chart applied to the pixel's position.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_paints_like_the_chunk_meshes() {
        let t = painted();
        let params = TerrainClipParams { origin: ORIGIN, cell: CELL, ..Default::default() };
        let fragment =
            "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { let p = local_cells(f.world); if (dropped(p)) { discard; } return paint_weights(p); }";
        let mut checked = 0;
        for (k, (p, xz)) in draw_from_above(&t, params, fragment).iter().enumerate() {
            if !on_ground(&t, *xz) {
                continue;
            }

            let want = chunk_weights(&t, *xz);
            assert!((0..4).all(|i| (f64::from(p[i]) - want[i]).abs() < 1e-3), "pixel {k} at {xz} painted {p:?}, want {want:?}");
            checked += 1;
        }

        assert!(checked > 10000, "{checked} pixels checked");
        let zero = t.vertex(12, 7);
        assert_eq!(chunk_weights(&t, DVec2::new(zero.x + 0.01, zero.z + 0.02)).map(|w| (w * 100.0).round()), [100.0, 0.0, 0.0, 0.0]);

        let rows = [[0.0011, 0.0004, -0.0007, 1.6], [-0.0004, -0.0002, 0.0013, -2.3]];
        let fragment = "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { if (dropped(local_cells(f.world))) { discard; } return vec4<f32>(lightmap_uv(f.world), f32(clip.grid.z), f32(clip.grid.w)); }";
        for chart in [Some(rows), None] {
            let params = TerrainClipParams { selected: chart.is_some(), chart, ..params };
            let mut checked = 0;
            for (k, (p, xz)) in draw_from_above(&t, params, fragment).iter().enumerate() {
                if !on_ground(&t, *xz) {
                    continue;
                }

                let at = DVec3::new(xz.x, t.height_at(xz.x, xz.y).unwrap(), xz.y);
                let want = match chart {
                    Some(rows) => rows.map(|r| r[0] as f64 * at.x + r[1] as f64 * at.y + r[2] as f64 * at.z + r[3] as f64).to_vec(),
                    None => vec![-1.0, -1.0],
                };
                let flags = [chart.is_some() as u8 as f32; 2];
                assert!((0..2).all(|i| (f64::from(p[i]) - want[i]).abs() < 1e-3) && [p[2], p[3]] == flags, "pixel {k} at {xz}: {p:?}, want {want:?}");
                checked += 1;
            }

            assert!(checked > 10000, "{checked} pixels checked with {chart:?}");
        }
    }

    /// The sun sees the terrain's surface from the rings around the viewer's eye, and the edge and both holes leave
    /// the shadow map clear so sunlight falls through them.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_casts_shadow_depth_except_through_holes() {
        let t = painted();
        let (device, queue) = device(false);
        let layout = bind_group_layout(&device);
        let mut clip = TerrainClipmap::new(&device, &queue, &layout, t.resolution).unwrap();
        clip.write_whole(&queue, &t.heights, &pack(&t)).unwrap();
        clip.set_params(&queue, &TerrainClipParams { origin: ORIGIN, cell: CELL, ..Default::default() });
        let size = t.size().as_vec2();
        let [low, high] = range(&t.heights);
        let sun = crate::Renderer::sun_view_proj(Vec3::NEG_Y, ORIGIN + Vec3::new(-40.0, low, -40.0), ORIGIN + Vec3::new(size.x + 40.0, high, size.y + 70.0));
        let eye = at_cells([30.5, 4.25], 200.0);
        let size = [512, 288];
        let groups = [None, None, Some((&layout, &clip.bind_group))];
        let depths = render_shadow_depths(&device, &queue, &crate::terrain_clip_shader(), &groups, &clip.instances(eye, sun), eye, sun, size);

        let [w, h] = size;
        let inverse = sun.inverse();
        let [mut ground, mut past, mut holes] = [0, 0, 0];
        for (k, &depth) in depths.iter().enumerate() {
            let ndc = Vec2::new(((k as u32 % w) as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - ((k as u32 / w) as f32 + 0.5) / h as f32 * 2.0);
            let at = inverse.project_point3(ndc.extend(depth)).as_dvec3();
            let xz = DVec2::new(at.x, at.z);
            let Some((inside, hole)) = spot(&t, xz) else { continue };
            assert_eq!(depth < 1.0, inside && !hole, "pixel {k} at {xz}: depth {depth}");
            if !inside {
                past += 1;
            } else if hole {
                holes += 1;
            } else {
                let want = t.height_at(xz.x, xz.y).unwrap();
                assert!((at.y - want).abs() < 0.05, "pixel {k} at {xz} cast from height {}, want {want}", at.y);
                ground += 1;
            }
        }

        assert!(ground > 30000 && past > 10000 && holes > 60, "{ground} ground, {past} past the edge and {holes} hole pixels checked");
    }
}
