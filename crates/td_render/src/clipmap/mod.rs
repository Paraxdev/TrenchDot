//! Terrain3D ground drawn as a geometry clipmap: a fixed set of grid meshes that follows the camera and reads its
//! heights on the GPU, the way Terrain3D draws it in Godot.

pub mod layout;
pub mod look;

use glam::{DVec2, I64Vec2, Mat4, Vec3};

use crate::wgpu;

/// Locations on each axis of Terrain3D's region map, which runs from [`LOC_MIN`] to `LOC_MIN + MAP_SIDE - 1`.
pub const MAP_SIDE: i32 = 32;
pub const LOC_MIN: i32 = -16;
const NO_REGION: i32 = -1;
const COARSE_ONLY: i32 = -2;

/// Budget for the fine arrays of one clipmap.
const FINE_BYTES: u64 = 384 << 20;
/// Bytes of one fine layer per vertex: a height, a control word and a color.
const FINE_TEXEL_BYTES: u64 = 12;
const MIN_LAYERS: u64 = 4;
const MAX_LAYERS: u64 = 64;
/// Widest coarse atlas, 32 regions of 128 texels.
const MAX_ATLAS_SIDE: u32 = 4096;

/// Constants the clipmap shader expects in front of it.
pub(crate) fn wgsl_consts() -> String {
    format!(
        "const MESH_SIZE: f32 = {}.0;\nconst MAP_SIDE: i32 = {MAP_SIDE};\nconst LOC_MIN: i32 = {LOC_MIN};\nconst NO_REGION: i32 = {NO_REGION};\nconst COARSE_ONLY: i32 = {COARSE_ONLY};\n{}",
        layout::MESH_SIZE,
        look::wgsl_consts()
    )
}

/// Fine layers a clipmap holds for a world of `regions` regions: as many as fit the budget, at least 4 and at most 64,
/// and never more than the world has, but always one so the arrays exist.
pub fn fine_capacity(region_size: u32, regions: usize) -> u32 {
    let layer = FINE_TEXEL_BYTES * region_size as u64 * region_size as u64;
    let k = (FINE_BYTES / layer.max(1)).clamp(MIN_LAYERS, MAX_LAYERS);
    k.min(regions as u64).max(1) as u32
}

/// Cells between the texels of the coarse atlas, so a region spans 64 or 128 texels.
pub fn coarse_stride(region_size: u32) -> u32 {
    if region_size <= 512 { 4 } else { 16 }
}

/// Where a location sits in the shader's region map, None outside Terrain3D's map.
pub fn region_index(loc: [i32; 2]) -> Option<usize> {
    let [x, z] = loc.map(|v| v - LOC_MIN);
    ((0..MAP_SIDE).contains(&x) && (0..MAP_SIDE).contains(&z)).then(|| (z * MAP_SIDE + x) as usize)
}

/// What the GPU holds of a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Absent,
    /// Only its block of the coarse atlas.
    Coarse,
    /// A layer of the fine arrays.
    Fine(u32),
}

impl Slot {
    fn encode(self) -> i32 {
        match self {
            Slot::Absent => NO_REGION,
            Slot::Coarse => COARSE_ONLY,
            Slot::Fine(layer) => layer as i32,
        }
    }

    fn decode(v: i32) -> Self {
        match v {
            COARSE_ONLY => Slot::Coarse,
            v if v >= 0 => Slot::Fine(v as u32),
            _ => Slot::Absent,
        }
    }
}

/// Terrain3D's world background, what draws where no region is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    /// Nothing, and the cells touching the edge of the regions are dropped like Terrain3D drops them.
    #[default]
    None,
    /// A plane at the ground level the region edges blend into.
    Flat,
    Noise,
}

/// The size of a clipmap's GPU arrays. Changing any of it means a new [`GpuClipmap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipmapDesc {
    pub region_size: u32,
    /// Fine layers, see [`fine_capacity`].
    pub layers: u32,
    /// Cells between coarse texels, see [`coarse_stride`].
    pub coarse_stride: u32,
    /// Lowest and highest region map location the coarse atlas covers.
    pub atlas: [[i32; 2]; 2],
}

impl ClipmapDesc {
    pub fn check(&self) -> Result<(), String> {
        let s = self.coarse_stride;
        if !self.region_size.is_power_of_two() || !s.is_power_of_two() || s >= self.region_size {
            return Err(format!("region size {} and coarse stride {s} must be powers of two with the stride smaller", self.region_size));
        }

        if !(1..=256).contains(&self.layers) {
            return Err(format!("{} fine layers is outside 1 to 256", self.layers));
        }

        let [lo, hi] = self.atlas;
        if region_index(lo).is_none() || region_index(hi).is_none() || lo[0] > hi[0] || lo[1] > hi[1] {
            return Err(format!("coarse atlas {lo:?} to {hi:?} is not a rectangle of the region map"));
        }

        let [w, h] = self.atlas_size();
        if w.max(h) > MAX_ATLAS_SIDE {
            return Err(format!("coarse atlas of {w} by {h} texels is wider than {MAX_ATLAS_SIDE}"));
        }

        Ok(())
    }

    /// Coarse texels along a region side.
    pub fn coarse_texels(&self) -> u32 {
        self.region_size / self.coarse_stride
    }

    pub fn atlas_size(&self) -> [u32; 2] {
        let [lo, hi] = self.atlas;
        [0, 1].map(|i| (hi[i] - lo[i] + 1) as u32 * self.coarse_texels())
    }

    fn in_atlas(&self, loc: [i32; 2]) -> bool {
        let [lo, hi] = self.atlas;
        (0..2).all(|i| (lo[i]..=hi[i]).contains(&loc[i]))
    }
}

/// How a clipmap draws, changeable at any time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipmapParams {
    /// Map units between vertices.
    pub cell: f32,
    /// Map units per meter, heights are meters.
    pub upm: f32,
    pub background: Background,
    /// Meters, what a Flat or Noise background blends the region edges toward.
    pub ground_level: f32,
    pub region_blend: f32,
    /// Lowest and highest map unit y the ground reaches, background included, for culling.
    pub heights: [f32; 2],
    pub selected: bool,
    /// Linear color of the wireframe lines.
    pub wire: [f32; 4],
    /// Sample the textures, or draw each with its mean color when false.
    pub textures: bool,
    /// Terrain3D's auto shader, None when the material has it off.
    pub auto_shader: Option<AutoShader>,
}

/// Terrain3D's auto shader: two textures mixed by slope and height wherever the control map asks for it or no region
/// is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoShader {
    pub slope: f32,
    pub height_reduction: f32,
    pub base: u32,
    pub overlay: u32,
}

impl Default for ClipmapParams {
    fn default() -> Self {
        Self {
            cell: 32.0,
            upm: 32.0,
            background: Background::None,
            ground_level: 0.0,
            region_blend: 0.25,
            heights: [0.0, 0.0],
            selected: false,
            wire: [1.0; 4],
            textures: true,
            auto_shader: None,
        }
    }
}

/// The clipmap shader's uniform, see clipmap.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ClipmapUniform {
    grid: [i32; 4],
    world: [f32; 4],
    look: [f32; 4],
    wire: [f32; 4],
    auto_shader: [f32; 4],
    regions: [[i32; 4]; 256],
    textures: [look::TextureUniform; look::MAX_TEXTURES],
}

impl ClipmapUniform {
    fn new(desc: &ClipmapDesc) -> Self {
        let mut u: Self = bytemuck::Zeroable::zeroed();
        u.grid = [desc.region_size as i32, desc.coarse_stride as i32, desc.atlas[0][0], desc.atlas[0][1]];
        u.regions = [[NO_REGION; 4]; 256];
        u.textures = [look::TextureUniform::default(); look::MAX_TEXTURES];
        u.set_params(&ClipmapParams::default(), false);
        u
    }

    /// `listed` is whether the world has textures at all, without any the ground keeps its plain color.
    fn set_params(&mut self, p: &ClipmapParams, listed: bool) {
        let background = match p.background {
            Background::None => 0.0,
            Background::Flat => 1.0,
            Background::Noise => 2.0,
        };
        let textures = match (listed, p.textures) {
            (false, _) => 0.0,
            (true, false) => 1.0,
            (true, true) => 2.0,
        };
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        self.world = [p.cell, p.upm, p.ground_level, p.region_blend];
        self.look = [background, flag(p.selected), textures, flag(p.auto_shader.is_some())];
        self.wire = p.wire;
        let a = p.auto_shader.unwrap_or(AutoShader { slope: 0.0, height_reduction: 0.0, base: 0, overlay: 0 });
        self.auto_shader = [a.slope, a.height_reduction, a.base.min(31) as f32, a.overlay.min(31) as f32];
    }

    fn slot(&self, i: usize) -> Slot {
        Slot::decode(self.regions[i / 4][i % 4])
    }
}

pub(crate) fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding: u32, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
        count: None,
    };
    let float = wgpu::TextureSampleType::Float { filterable: false };
    let filtered = wgpu::TextureSampleType::Float { filterable: true };
    let uint = wgpu::TextureSampleType::Uint;
    let (array, flat) = (wgpu::TextureViewDimension::D2Array, wgpu::TextureViewDimension::D2);
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("clipmap"),
        entries: &[
            texture(0, float, array),
            texture(1, uint, array),
            texture(2, float, array),
            texture(3, float, flat),
            texture(4, uint, flat),
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            texture(6, filtered, array),
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// One Terrain3D world on the GPU: fine region layers, the coarse atlas and the region map. Fill it with
/// [`Self::write_fine`] and [`Self::write_coarse`], then mark the regions drawable with [`Self::set_slots`].
pub struct GpuClipmap {
    desc: ClipmapDesc,
    fine_heights: wgpu::Texture,
    fine_control: wgpu::Texture,
    fine_color: wgpu::Texture,
    coarse_heights: wgpu::Texture,
    coarse_control: wgpu::Texture,
    albedo: wgpu::Texture,
    sampler: wgpu::Sampler,
    /// Whether [`Self::set_textures`] was given any.
    listed: bool,
    uniform: ClipmapUniform,
    params: ClipmapParams,
    buffer: wgpu::Buffer,
    pub(crate) bind_group: wgpu::BindGroup,
}

impl GpuClipmap {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, desc: &ClipmapDesc) -> Result<Self, String> {
        desc.check()?;
        let texture = |label: &str, [w, h]: [u32; 2], layers: u32, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let side = [desc.region_size; 2];
        let fine_heights = texture("clipmap fine heights", side, desc.layers, wgpu::TextureFormat::R32Float);
        let fine_control = texture("clipmap fine control", side, desc.layers, wgpu::TextureFormat::R32Uint);
        // Terrain3D reads its color maps as sRGB.
        let fine_color = texture("clipmap fine color", side, desc.layers, wgpu::TextureFormat::Rgba8UnormSrgb);
        let coarse_heights = texture("clipmap coarse heights", desc.atlas_size(), 1, wgpu::TextureFormat::R32Float);
        let coarse_control = texture("clipmap coarse control", desc.atlas_size(), 1, wgpu::TextureFormat::R32Uint);
        let uniform = ClipmapUniform::new(desc);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clipmap"),
            size: std::mem::size_of::<ClipmapUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let albedo = albedo_array(device, queue, &look::AlbedoLayers::new(&[]));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clipmap albedo"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let data = [&fine_heights, &fine_control, &fine_color, &coarse_heights, &coarse_control, &albedo];
        let bind_group = bind_group(device, layout, data, &buffer, &sampler);
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&uniform));
        Ok(GpuClipmap {
            desc: *desc,
            fine_heights,
            fine_control,
            fine_color,
            coarse_heights,
            coarse_control,
            albedo,
            sampler,
            listed: false,
            uniform,
            params: ClipmapParams::default(),
            buffer,
            bind_group,
        })
    }

    /// Replaces the textures the ground draws with.
    pub(crate) fn set_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, textures: &[look::ClipmapTexture]) {
        let layers = look::AlbedoLayers::new(textures);
        self.albedo = albedo_array(device, queue, &layers);
        self.listed = !textures.is_empty();
        self.uniform.textures = layers.entries;
        self.uniform.set_params(&self.params, self.listed);
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniform));
        let data = [&self.fine_heights, &self.fine_control, &self.fine_color, &self.coarse_heights, &self.coarse_control, &self.albedo];
        self.bind_group = bind_group(device, layout, data, &self.buffer, &self.sampler);
    }

    pub fn desc(&self) -> &ClipmapDesc {
        &self.desc
    }

    pub fn params(&self) -> &ClipmapParams {
        &self.params
    }

    pub fn slot(&self, loc: [i32; 2]) -> Slot {
        region_index(loc).map_or(Slot::Absent, |i| self.uniform.slot(i))
    }

    pub fn set_params(&mut self, queue: &wgpu::Queue, params: &ClipmapParams) {
        self.params = *params;
        self.uniform.set_params(params, self.listed);
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniform));
    }

    /// Marks what the GPU holds of each region. Changes nothing when one of them names a layer or atlas block the
    /// clipmap does not have. Call it once the data is written, so a region never draws half uploaded.
    pub fn set_slots(&mut self, queue: &wgpu::Queue, slots: &[([i32; 2], Slot)]) -> Result<(), String> {
        let mut regions = self.uniform.regions;
        for &(loc, slot) in slots {
            let i = region_index(loc).ok_or_else(|| format!("region {loc:?} is outside the region map"))?;
            match slot {
                Slot::Fine(layer) if layer >= self.desc.layers => return Err(format!("fine layer {layer} of region {loc:?} is past {}", self.desc.layers)),
                Slot::Coarse if !self.desc.in_atlas(loc) => return Err(format!("region {loc:?} is outside the coarse atlas")),
                _ => regions[i / 4][i % 4] = slot.encode(),
            }
        }

        self.uniform.regions = regions;
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniform));
        Ok(())
    }

    /// Writes a rectangle of a fine layer, `origin` and `size` in cells of the region, the slices row by row.
    #[allow(clippy::too_many_arguments)]
    pub fn write_fine(
        &self,
        queue: &wgpu::Queue,
        layer: u32,
        origin: [u32; 2],
        size: [u32; 2],
        heights: &[f32],
        control: &[u32],
        color: &[[u8; 4]],
    ) -> Result<(), String> {
        let n = size[0] as usize * size[1] as usize;
        if layer >= self.desc.layers {
            return Err(format!("fine layer {layer} is past {}", self.desc.layers));
        }

        if (0..2).any(|i| origin[i].checked_add(size[i]).is_none_or(|end| end > self.desc.region_size)) {
            return Err(format!("rectangle {origin:?} of {size:?} leaves a region of {}", self.desc.region_size));
        }

        if heights.len() != n || control.len() != n || color.len() != n {
            return Err(format!("{} heights, {} control words and {} colors for {n} cells", heights.len(), control.len(), color.len()));
        }

        let at = wgpu::Origin3d { x: origin[0], y: origin[1], z: layer };
        write(queue, &self.fine_heights, at, size, bytemuck::cast_slice(heights));
        write(queue, &self.fine_control, at, size, bytemuck::cast_slice(control));
        write(queue, &self.fine_color, at, size, bytemuck::cast_slice(color));
        Ok(())
    }

    /// Writes a region's block of the coarse atlas, [`ClipmapDesc::coarse_texels`] squared values row by row.
    pub fn write_coarse(&self, queue: &wgpu::Queue, loc: [i32; 2], heights: &[f32], control: &[u32]) -> Result<(), String> {
        if !self.desc.in_atlas(loc) {
            return Err(format!("region {loc:?} is outside the coarse atlas"));
        }

        let cs = self.desc.coarse_texels();
        let n = (cs * cs) as usize;
        if heights.len() != n || control.len() != n {
            return Err(format!("{} heights and {} control words for {n} coarse texels", heights.len(), control.len()));
        }

        let lo = self.desc.atlas[0];
        let at = wgpu::Origin3d { x: (loc[0] - lo[0]) as u32 * cs, y: (loc[1] - lo[1]) as u32 * cs, z: 0 };
        write(queue, &self.coarse_heights, at, [cs, cs], bytemuck::cast_slice(heights));
        write(queue, &self.coarse_control, at, [cs, cs], bytemuck::cast_slice(control));
        Ok(())
    }

    /// Lowest and highest corner in cells of the present regions, None when there are none.
    fn area(&self) -> Option<[I64Vec2; 2]> {
        let size = self.desc.region_size as i64;
        let mut out: Option<[I64Vec2; 2]> = None;
        for i in 0..(MAP_SIDE * MAP_SIDE) as usize {
            if self.uniform.slot(i) == Slot::Absent {
                continue;
            }

            let loc = I64Vec2::new((i as i32 % MAP_SIDE + LOC_MIN) as i64, (i as i32 / MAP_SIDE + LOC_MIN) as i64);
            let [lo, hi] = [loc * size, (loc + 1) * size];
            out = Some(out.map_or([lo, hi], |[a, b]| [a.min(lo), b.max(hi)]));
        }

        out
    }

    /// The parts a view at `eye` draws. Without a background nothing draws past the regions.
    pub fn instances(&self, eye: Vec3, view_proj: Mat4) -> layout::Instances {
        let cell = self.params.cell as f64;
        let area = match self.params.background {
            Background::None => Some(self.area().unwrap_or([I64Vec2::ZERO; 2])),
            _ => None,
        };
        let cull = layout::Cull { frustum: Some(layout::Frustum::new(view_proj)), cell, heights: self.params.heights.map(|h| h as f64), area };
        layout::Instances::new(DVec2::new(eye.x as f64, eye.z as f64) / cell, &cull)
    }
}

/// `data` holds the fine heights, control and color, the coarse heights and control, and the albedo, in binding order.
fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    data: [&wgpu::Texture; 6],
    buffer: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let views: Vec<wgpu::TextureView> = data
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let dimension = if i == 3 || i == 4 { wgpu::TextureViewDimension::D2 } else { wgpu::TextureViewDimension::D2Array };
            t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() })
        })
        .collect();
    let binding = |i: usize| if i < 5 { i as u32 } else { i as u32 + 1 };
    let mut entries: Vec<wgpu::BindGroupEntry> =
        views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry { binding: binding(i), resource: wgpu::BindingResource::TextureView(v) }).collect();
    entries.push(wgpu::BindGroupEntry { binding: 5, resource: buffer.as_entire_binding() });
    entries.push(wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(sampler) });
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("clipmap"), layout, entries: &entries })
}

/// The albedo array with its mips, a layer per texture id.
fn albedo_array(device: &wgpu::Device, queue: &wgpu::Queue, layers: &look::AlbedoLayers) -> wgpu::Texture {
    let side = layers.side;
    let mips = side.ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clipmap albedo"),
        size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: layers.layers.len() as u32 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (z, layer) in layers.layers.iter().enumerate() {
        for (mip, level) in crate::mip_chain(layer, mips).iter().enumerate() {
            let origin = wgpu::Origin3d { x: 0, y: 0, z: z as u32 };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip as u32, origin, aspect: wgpu::TextureAspect::All },
                level.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(level.width() * 4), rows_per_image: Some(level.height()) },
                wgpu::Extent3d { width: level.width(), height: level.height(), depth_or_array_layers: 1 },
            );
        }
    }

    texture
}

/// Every clipmap texture has 4 byte texels.
fn write(queue: &wgpu::Queue, texture: &wgpu::Texture, origin: wgpu::Origin3d, [w, h]: [u32; 2], data: &[u8]) {
    if w == 0 || h == 0 {
        return;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use glam::camera::rh::proj::directx::orthographic;
    use glam::camera::rh::view::look_to_mat4;

    use super::*;

    const DESC: ClipmapDesc = ClipmapDesc { region_size: 64, layers: 3, coarse_stride: 4, atlas: [[-5, 1], [-2, 4]] };

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut cx = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }

            std::thread::yield_now();
        }
    }

    fn device(noop: bool) -> (wgpu::Device, wgpu::Queue) {
        let descriptor = if noop {
            let mut d = wgpu::InstanceDescriptor::new_without_display_handle();
            d.backends = wgpu::Backends::NOOP;
            d.backend_options.noop.enable = true;
            d
        } else {
            wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        };
        let instance = wgpu::Instance::new(descriptor);
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
    }

    fn top_down(eye: Vec3, half: [f32; 2]) -> Mat4 {
        orthographic(-half[0], half[0], -half[1], half[1], 1.0, 100_000.0) * look_to_mat4(eye, Vec3::NEG_Y, Vec3::NEG_Z)
    }

    #[test]
    fn fine_capacity_fits_the_budget_between_its_clamps() {
        let cases = [
            ((64, 1000), 64),
            ((724, 1000), 64),
            ((725, 1000), 63),
            ((1024, 1000), 32),
            ((2048, 1000), 8),
            ((2590, 1000), 5),
            ((2896, 1000), 4),
            ((3000, 1000), 4),
            ((8192, 1000), 4),
            ((1024, 33), 32),
            ((1024, 32), 32),
            ((1024, 3), 3),
            ((1024, 1), 1),
            ((1024, 0), 1),
        ];
        for ((size, regions), want) in cases {
            assert_eq!(fine_capacity(size, regions), want, "{size} with {regions} regions");
        }

        assert_eq!([256, 512, 1024].map(coarse_stride), [4, 4, 16]);
    }

    #[test]
    fn region_map_indices_run_row_by_row() {
        let cases = [([-16, -16], Some(0)), ([15, -16], Some(31)), ([-16, 15], Some(992)), ([15, 15], Some(1023)), ([3, -7], Some(9 * 32 + 19))];
        for (loc, want) in cases {
            assert_eq!(region_index(loc), want, "{loc:?}");
        }

        for loc in [[16, 0], [-17, 0], [0, 16], [0, -17]] {
            assert_eq!(region_index(loc), None, "{loc:?}");
        }
    }

    #[test]
    fn descs_outside_the_limits_are_refused() {
        assert_eq!(DESC.check(), Ok(()));
        assert_eq!((DESC.coarse_texels(), DESC.atlas_size()), (16, [64, 64]));
        let widest = ClipmapDesc { region_size: 2048, layers: 256, coarse_stride: 16, atlas: [[-16, -16], [15, 15]] };
        assert_eq!((widest.check(), widest.atlas_size()), (Ok(()), [4096, 4096]));
        let small = ClipmapDesc { layers: 1, coarse_stride: 32, atlas: [[7, -3], [7, -3]], ..DESC };
        assert_eq!((small.check(), small.atlas_size()), (Ok(()), [2, 2]));
        let bad = [
            ClipmapDesc { region_size: 1000, ..DESC },
            ClipmapDesc { coarse_stride: 3, ..DESC },
            ClipmapDesc { coarse_stride: 64, ..DESC },
            ClipmapDesc { layers: 0, ..DESC },
            ClipmapDesc { layers: 257, ..DESC },
            ClipmapDesc { atlas: [[-2, 1], [-5, 4]], ..DESC },
            ClipmapDesc { atlas: [[-5, 4], [-2, 1]], ..DESC },
            ClipmapDesc { atlas: [[-17, 1], [-2, 4]], ..DESC },
            ClipmapDesc { atlas: [[-5, 1], [-2, 16]], ..DESC },
            ClipmapDesc { coarse_stride: 8, ..widest },
        ];
        for desc in bad {
            assert!(desc.check().is_err(), "{desc:?}");
        }
    }

    #[test]
    fn writes_and_slots_outside_the_arrays_are_refused() {
        let (device, queue) = device(true);
        let mut clip = GpuClipmap::new(&device, &queue, &bind_group_layout(&device), &DESC).unwrap();
        assert!(GpuClipmap::new(&device, &queue, &bind_group_layout(&device), &ClipmapDesc { layers: 0, ..DESC }).is_err());
        let n = 64 * 64;
        let (h, c, k) = (vec![0.5f32; n], vec![1u32; n], vec![[255u8; 4]; n]);
        let fine = |layer, origin, size: [u32; 2], short: usize| {
            let m = (size[0] * size[1]) as usize;
            clip.write_fine(&queue, layer, origin, size, &h[..m], &c[short..m], &k[..m])
        };
        assert_eq!(fine(2, [0, 0], [64, 64], 0), Ok(()));
        assert!(fine(3, [0, 0], [64, 64], 0).is_err(), "a layer past the last");
        assert_eq!(fine(0, [60, 10], [4, 54], 0), Ok(()), "a rectangle ending on the far corner");
        assert!(fine(0, [61, 10], [4, 54], 0).is_err(), "one cell past x");
        assert!(fine(0, [60, 11], [4, 54], 0).is_err(), "one cell past z");
        assert!(fine(0, [u32::MAX, 0], [2, 1], 0).is_err(), "an origin that overflows");
        assert!(fine(0, [0, 0], [64, 64], 1).is_err(), "one control word short");
        assert_eq!(fine(1, [5, 5], [0, 0], 0), Ok(()), "an empty rectangle writes nothing");

        let (ch, cc) = (vec![0.0f32; 256], vec![0u32; 256]);
        assert_eq!(clip.write_coarse(&queue, [-5, 1], &ch, &cc), Ok(()));
        assert_eq!(clip.write_coarse(&queue, [-2, 4], &ch, &cc), Ok(()));
        for loc in [[-1, 4], [-5, 0], [-6, 2], [-3, 5]] {
            assert!(clip.write_coarse(&queue, loc, &ch, &cc).is_err(), "{loc:?} is outside the atlas");
        }

        assert!(clip.write_coarse(&queue, [-5, 1], &ch[..255], &cc).is_err());

        clip.set_slots(&queue, &[([-5, 1], Slot::Fine(2)), ([-2, 4], Slot::Coarse), ([15, -16], Slot::Fine(0))]).unwrap();
        let got = [[-5, 1], [-2, 4], [15, -16], [0, 0], [16, 0]].map(|l| clip.slot(l));
        assert_eq!(got, [Slot::Fine(2), Slot::Coarse, Slot::Fine(0), Slot::Absent, Slot::Absent]);
        for bad in [([0, 0], Slot::Fine(3)), ([-1, 4], Slot::Coarse), ([16, 0], Slot::Absent)] {
            assert!(clip.set_slots(&queue, &[([3, 3], Slot::Fine(1)), bad]).is_err(), "{bad:?}");
            assert_eq!(clip.slot([3, 3]), Slot::Absent, "a refused batch changes nothing");
        }

        clip.set_slots(&queue, &[([-5, 1], Slot::Absent)]).unwrap();
        assert_eq!(clip.slot([-5, 1]), Slot::Absent);
    }

    #[test]
    fn no_background_draws_only_over_the_regions() {
        let (device, queue) = device(true);
        let mut clip = GpuClipmap::new(&device, &queue, &bind_group_layout(&device), &DESC).unwrap();
        let eye = Vec3::new(-3000.0, 4000.0, 2500.0);
        let view_proj = top_down(eye, [2.0e6, 2.0e6]);
        let params = ClipmapParams { cell: 24.0, heights: [-500.0, 900.0], ..Default::default() };
        clip.set_params(&queue, &params);
        assert!(clip.instances(eye, view_proj).parts.is_empty(), "nothing draws without regions");

        let overlaps = |parts: &[layout::Part], lo: I64Vec2, hi: I64Vec2| parts.iter().all(|p| p.cells()[0].cmplt(hi).all() && p.cells()[1].cmpgt(lo).all());
        clip.set_slots(&queue, &[([-3, 2], Slot::Coarse)]).unwrap();
        let one = clip.instances(eye, view_proj).parts;
        assert!(!one.is_empty());
        assert!(overlaps(&one, I64Vec2::new(-192, 128), I64Vec2::new(-128, 192)));

        clip.set_slots(&queue, &[([-2, 4], Slot::Fine(0))]).unwrap();
        let two = clip.instances(eye, view_proj).parts;
        assert!(two.len() > one.len());
        assert!(overlaps(&two, I64Vec2::new(-192, 128), I64Vec2::new(-64, 320)));

        clip.set_params(&queue, &ClipmapParams { background: Background::Flat, ..params });
        assert_eq!(clip.instances(eye, view_proj).parts.len(), layout::ring(DVec2::new(-125.0, 2500.0 / 24.0)).len());
    }

    /// Draws `clip` seen from `eye` into a `w` by `h` Rgba32Float image with `fragment`, a fragment entry `fs_test`
    /// added to the clipmap shader, and reads the pixels back row by row. `w` has to be a multiple of 16, since wgpu
    /// copies rows of 256 bytes.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        clip: &GpuClipmap,
        eye: Vec3,
        view_proj: Mat4,
        [w, h]: [u32; 2],
        fragment: &str,
    ) -> Vec<[f32; 4]> {
        let camera = crate::CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            eye: eye.extend(1.0).to_array(),
            params: [0.0, 0.0, 1.0, 1.0],
            viewport: [w as f32, h as f32, 1.0, 0.0],
        };
        use wgpu::util::DeviceExt;
        let buffer = |contents: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents, usage });
        let camera = buffer(bytemuck::bytes_of(&camera), wgpu::BufferUsages::UNIFORM);
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() }],
        });
        let source = format!("{}\n{fragment}", crate::clipmap_shader());
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_bgl), Some(layout)],
            immediate_size: 0,
        });
        let vertex_attrs = wgpu::vertex_attr_array![0 => Uint32x2];
        let instance_attrs = wgpu::vertex_attr_array![1 => Sint32x2, 2 => Uint32];
        let format = wgpu::TextureFormat::Rgba32Float;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex_attrs }),
                    Some(wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Instance, attributes: &instance_attrs }),
                ],
            },
            // The renderer's culling, so ground wound the wrong way leaves the image empty.
            primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_test"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let instances = clip.instances(eye, view_proj);
        let records: Vec<layout::ClipmapInstance> = instances.parts.iter().map(|&p| p.into()).collect();
        let records = buffer(bytemuck::cast_slice(&records), wgpu::BufferUsages::VERTEX);
        let meshes: Vec<_> = layout::MeshKind::ALL
            .iter()
            .map(|kind| {
                let indices = kind.indices();
                let vertex = buffer(bytemuck::cast_slice(&kind.vertices()), wgpu::BufferUsages::VERTEX);
                (vertex, buffer(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX), indices.len() as u32)
            })
            .collect();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 16) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &camera_bg, &[]);
            pass.set_bind_group(1, &clip.bind_group, &[]);
            pass.set_vertex_buffer(1, records.slice(..));
            for ((vertex, index, count), range) in meshes.iter().zip(&instances.ranges) {
                pass.set_vertex_buffer(0, vertex.slice(..));
                pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, range.clone());
            }
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 16), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        bytemuck::cast_slice::<u8, [f32; 4]>(&readback.slice(..).get_mapped_range().unwrap()).to_vec()
    }

    /// The cell under the center of every pixel of a [`draw`] image, row by row.
    fn pixel_cells(view_proj: Mat4, [w, h]: [u32; 2], cell: f32) -> Vec<[f64; 2]> {
        let inverse = view_proj.inverse();
        (0..w * h)
            .map(|i| {
                let (col, row) = ((i % w) as f32, (i / w) as f32);
                let at = inverse.project_point3(Vec3::new((col + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (row + 0.5) / h as f32 * 2.0, 0.5));
                [at.x / cell, at.z / cell].map(f64::from)
            })
            .collect()
    }

    /// Draws a slanted plane held by a fine region and a coarse one beside it, with no region around them, and reads
    /// back the world position of every pixel.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_draws_the_heights_it_was_given() {
        let (device, queue) = device(false);
        let layout = bind_group_layout(&device);
        let desc = ClipmapDesc { region_size: 64, layers: 2, coarse_stride: 4, atlas: [[-4, 1], [-2, 2]] };
        let mut clip = GpuClipmap::new(&device, &queue, &layout, &desc).unwrap();
        let plane = |x: f64, z: f64| 3.0 + 0.25 * x - 0.125 * z;
        let n = 64 * 64;
        let fine: Vec<f32> = (0..n as i64).map(|k| plane((-192 + k % 64) as f64, (128 + k / 64) as f64) as f32).collect();
        let (c, k) = (vec![0u32; n], vec![[255u8; 4]; n]);
        clip.write_fine(&queue, 1, [0, 0], [64, 64], &fine, &c, &k).unwrap();
        clip.write_fine(&queue, 0, [0, 0], [64, 64], &vec![1000.0; n], &c, &k).unwrap();
        let coarse: Vec<f32> = (0..256i64).map(|k| plane((-128 + k % 16 * 4) as f64, (128 + k / 16 * 4) as f64) as f32).collect();
        clip.write_coarse(&queue, [-2, 2], &coarse, &[0; 256]).unwrap();
        clip.write_coarse(&queue, [-3, 2], &[-1000.0; 256], &[0; 256]).unwrap();
        clip.set_slots(&queue, &[([-3, 2], Slot::Fine(1)), ([-2, 2], Slot::Coarse)]).unwrap();
        let (cell, upm) = (24.0f32, 40.0f32);
        clip.set_params(&queue, &ClipmapParams { cell, upm, heights: [-4000.0, 0.0], ..Default::default() });

        let [w, h] = [128u32, 64];
        let eye = Vec3::new(-128.0 * cell, 2000.0, 156.0 * cell);
        let view_proj = top_down(eye, [88.0 * cell, 44.0 * cell]);
        let fragment = "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { if (outside(f.world)) { discard; } return vec4<f32>(f.world, 1.0); }";
        let pixels = draw(&device, &queue, &layout, &clip, eye, view_proj, [w, h], fragment);

        let [mut fine_seen, mut coarse_seen] = [0, 0];
        for (i, (p, [x, z])) in pixels.iter().zip(pixel_cells(view_proj, [w, h], cell)).enumerate() {
            let drawn = (-192.0..-65.0).contains(&x) && (128.0..191.0).contains(&z);
            assert_eq!(p[3] == 1.0, drawn, "pixel {i} at cell {x} {z}: {p:?}");
            if !drawn {
                continue;
            }

            assert!((f64::from(p[0] / cell) - x).abs() < 0.01 && (f64::from(p[2] / cell) - z).abs() < 0.01, "pixel {i} at cell {x} {z} drew {p:?}");
            if (-188.0..-69.0).contains(&x) && (132.0..187.0).contains(&z) {
                let (got, want) = (f64::from(p[1] / upm), plane(x, z));
                assert!((got - want).abs() < 0.01, "pixel {i} at cell {x} {z}: {got} m, want {want}");
                *if x < -128.0 { &mut fine_seen } else { &mut coarse_seen } += 1;
            }
        }

        assert!(fine_seen > 500 && coarse_seen > 500, "{fine_seen} fine and {coarse_seen} coarse pixels checked");
    }

    #[test]
    fn texture_and_auto_settings_reach_the_uniform() {
        let mut u = ClipmapUniform::new(&DESC);
        assert_eq!((u.look, u.auto_shader), ([0.0; 4], [0.0; 4]), "no textures listed draws the plain ground");
        let auto = AutoShader { slope: 1.5, height_reduction: 0.25, base: 7, overlay: 40 };
        let params = ClipmapParams { background: Background::Flat, selected: true, auto_shader: Some(auto), ..Default::default() };
        u.set_params(&params, false);
        assert_eq!((u.look, u.auto_shader), ([1.0, 1.0, 0.0, 1.0], [1.5, 0.25, 7.0, 31.0]), "a texture id past 31 is clamped");
        u.set_params(&params, true);
        assert_eq!(u.look[2], 2.0);
        u.set_params(&ClipmapParams { textures: false, auto_shader: None, ..params }, true);
        assert_eq!((u.look, u.auto_shader), ([1.0, 1.0, 1.0, 0.0], [0.0; 4]));
    }

    #[test]
    fn set_textures_keeps_the_params() {
        let (device, queue) = device(true);
        let layout = bind_group_layout(&device);
        let mut clip = GpuClipmap::new(&device, &queue, &layout, &DESC).unwrap();
        let params = ClipmapParams { textures: false, ..Default::default() };
        clip.set_params(&queue, &params);
        let texture = look::ClipmapTexture { id: 2, albedo: look::Albedo::None, color: [0.2, 0.4, 0.6], uv_scale: 0.5, detiling: [0.0; 2] };
        clip.set_textures(&device, &queue, &layout, &[texture]);
        assert_eq!((clip.uniform.look[2], clip.uniform.textures[2].color, *clip.params()), (1.0, [0.2, 0.4, 0.6, 1.0], params));
        assert_eq!((clip.albedo.size().depth_or_array_layers, clip.albedo.size().width, clip.albedo.mip_level_count()), (3, 1, 1));
        clip.set_textures(&device, &queue, &layout, &[]);
        assert_eq!((clip.uniform.look[2], clip.uniform.textures[2]), (0.0, look::TextureUniform::default()));
    }

    /// One fine region off the origin painted with two solid textures: plain base cells, a block blending in the
    /// overlay, a tinted block of the color map and one hole vertex, read back as the albedo of every pixel.
    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_paints_blends_and_holes() {
        let (device, queue) = device(false);
        let layout = bind_group_layout(&device);
        let desc = ClipmapDesc { region_size: 64, layers: 1, coarse_stride: 4, atlas: [[2, -3], [2, -3]] };
        let mut clip = GpuClipmap::new(&device, &queue, &layout, &desc).unwrap();
        let origin = [128i64, -192];
        let hole = [44i64, 30];
        let blended = |l: [i64; 2]| (8..24).contains(&l[0]) && (8..56).contains(&l[1]);
        let tinted = |l: [i64; 2]| (48..60).contains(&l[0]) && (40..60).contains(&l[1]);
        let control = |l: [i64; 2]| {
            if l == hole {
                1 << 2
            } else if blended(l) {
                (1 << 22) | (128 << 14)
            } else {
                0
            }
        };
        let local = |k: usize| [(k % 64) as i64, (k / 64) as i64];
        let heights: Vec<f32> = (0..64 * 64).map(|k| (2.0 + 0.03 * local(k)[0] as f64 - 0.02 * local(k)[1] as f64) as f32).collect();
        let controls: Vec<u32> = (0..64 * 64).map(|k| control(local(k))).collect();
        let colors: Vec<[u8; 4]> = (0..64 * 64).map(|k| if tinted(local(k)) { [255, 128, 255, 255] } else { [255; 4] }).collect();
        clip.write_fine(&queue, 0, [0, 0], [64, 64], &heights, &controls, &colors).unwrap();
        clip.set_slots(&queue, &[([2, -3], Slot::Fine(0))]).unwrap();
        let (cell, upm) = (48.0f32, 32.0f32);
        clip.set_params(&queue, &ClipmapParams { cell, upm, heights: [0.0, 300.0], ..Default::default() });
        let solid = |rgb: [u8; 3]| look::Albedo::Thumbnail(image::RgbaImage::from_pixel(64, 64, image::Rgba([rgb[0], rgb[1], rgb[2], 90])));
        let textures = [
            look::ClipmapTexture { id: 0, albedo: solid([200, 40, 40]), color: [1.0; 3], uv_scale: 0.1, detiling: [0.3, 0.2] },
            look::ClipmapTexture { id: 1, albedo: solid([30, 60, 220]), color: [0.5, 1.0, 1.0], uv_scale: 0.7, detiling: [0.0; 2] },
        ];
        clip.set_textures(&device, &queue, &layout, &textures);

        // Off the cell grid by a non dyadic amount, so no pixel center lands on a cell edge or diagonal.
        let [w, h] = [144u32, 144];
        let eye = Vec3::new(160.1 * cell, 2000.0, -160.2 * cell);
        let view_proj = top_down(eye, [36.0 * cell, 36.0 * cell]);
        let fragment = "@fragment fn fs_test(f: VOut) -> @location(0) vec4<f32> { let m = f.world.xz / clip.world.y; let ddx = dpdx(m); let ddy = dpdy(m); if (dropped(f.world)) { discard; } return vec4<f32>(ground_albedo(f.world, normalize(f.normal), ddx, ddy), 1.0); }";
        let pixels = draw(&device, &queue, &layout, &clip, eye, view_proj, [w, h], fragment);

        let linear = |rgb: [u8; 3], tint: [f32; 3]| [0, 1, 2].map(|i| crate::srgb_to_linear(rgb[i] as f32 / 255.0) * tint[i]);
        let (red, blue) = (linear([200, 40, 40], [1.0; 3]), linear([30, 60, 220], [0.5, 1.0, 1.0]));
        let inside = |l: [i64; 2]| (0..64).contains(&l[0]) && (0..64).contains(&l[1]);
        let [mut base_seen, mut blend_seen, mut tint_seen, mut hole_seen] = [0; 4];
        for (i, (p, [x, z])) in pixels.iter().zip(pixel_cells(view_proj, [w, h], cell)).enumerate() {
            let c = [x.floor() as i64 - origin[0], z.floor() as i64 - origin[1]];
            let f = [x - x.floor(), z - z.floor()];
            let triangle = if (c[0] + c[1]).rem_euclid(2) == 0 {
                [[0, 0], [1, 1], if f[1] >= f[0] { [0, 1] } else { [1, 0] }]
            } else {
                [[0, 1], [1, 0], if f[0] + f[1] < 1.0 { [0, 0] } else { [1, 1] }]
            };
            let corners = triangle.map(|o| [c[0] + o[0], c[1] + o[1]]);
            let holed = corners.contains(&hole);
            if !corners.iter().all(|&l| inside(l)) || holed {
                hole_seen += holed as usize;
                assert_eq!(p[3], 0.0, "pixel {i} at cell {x} {z} should be dropped: {p:?}");
                continue;
            }

            let mut want = [0.0f32; 3];
            for (o, weight) in [([0, 0], (1.0 - f[0]) * (1.0 - f[1])), ([1, 0], f[0] * (1.0 - f[1])), ([0, 1], (1.0 - f[0]) * f[1]), ([1, 1], f[0] * f[1])] {
                let l = [c[0] + o[0], c[1] + o[1]];
                let blend = if blended(l) { 128.0 / 255.0 } else { 0.0 };
                let tint = if tinted(l) { [1.0, crate::srgb_to_linear(128.0 / 255.0), 1.0] } else { [1.0; 3] };
                for k in 0..3 {
                    want[k] += weight as f32 * (red[k] + (blue[k] - red[k]) * blend) * tint[k];
                }
            }

            let off = (0..3).map(|k| (p[k] - want[k]).abs()).fold(0.0, f32::max);
            assert!(p[3] == 1.0 && off < 0.004, "pixel {i} at cell {x} {z}: {p:?}, want {want:?}");
            let corners_of = |test: &dyn Fn([i64; 2]) -> bool| [[0, 0], [1, 1]].iter().all(|o| test([c[0] + o[0], c[1] + o[1]]));
            base_seen += corners_of(&|l| !blended(l) && !tinted(l)) as usize;
            blend_seen += corners_of(&blended) as usize;
            tint_seen += corners_of(&tinted) as usize;
        }

        assert!(base_seen > 2000 && blend_seen > 500 && tint_seen > 200 && hole_seen == 16, "{base_seen} {blend_seen} {tint_seen} {hole_seen}");
    }
}
