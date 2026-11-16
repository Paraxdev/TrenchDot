//! What the clipmap paints on the ground: Terrain3D's textures blended by its control map, the way its shader does.

use glam::DVec2;

/// Texture ids a control word can name.
pub const MAX_TEXTURES: usize = 32;
/// Side of an albedo layer when every texture has its full albedo.
pub const ALBEDO_SIDE: u32 = 512;
/// Side of the thumbnails Godot saves.
pub const THUMBNAIL_SIDE: u32 = 64;

const BASE_SHIFT: u32 = 27;
const OVERLAY_SHIFT: u32 = 22;
const BLEND_SHIFT: u32 = 14;
const ANGLE_SHIFT: u32 = 10;
const SCALE_SHIFT: u32 = 7;
const HOLE_BIT: u32 = 2;
const AUTO_BIT: u32 = 0;
/// What an id the texture list leaves out draws with, like the mesh terrain's stand in.
const UNLISTED_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
const DEFAULT_UV_SCALE: f32 = 0.1;

/// The control word layout the clipmap shader decodes with.
pub(crate) fn wgsl_consts() -> String {
    format!(
        "const BASE_SHIFT: u32 = {BASE_SHIFT}u;\nconst OVERLAY_SHIFT: u32 = {OVERLAY_SHIFT}u;\nconst BLEND_SHIFT: u32 = {BLEND_SHIFT}u;\nconst ANGLE_SHIFT: u32 = {ANGLE_SHIFT}u;\nconst SCALE_SHIFT: u32 = {SCALE_SHIFT}u;\nconst HOLE_BIT: u32 = {HOLE_BIT}u;\nconst AUTO_BIT: u32 = {AUTO_BIT}u;\n"
    )
}

/// A control word as the clipmap shader reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoded {
    pub base: u32,
    pub overlay: u32,
    /// How much of the overlay shows, 0 to 1.
    pub blend: f32,
    /// Radians the uv turns by.
    pub angle: f64,
    /// What the uv is multiplied by.
    pub scale: f64,
    pub hole: bool,
    pub auto: bool,
}

pub fn decode(control: u32) -> Decoded {
    let field = |shift: u32, bits: u32| (control >> shift) & ((1 << bits) - 1);
    Decoded {
        base: field(BASE_SHIFT, 5),
        overlay: field(OVERLAY_SHIFT, 5),
        blend: field(BLEND_SHIFT, 8) as f32 / 255.0,
        angle: field(ANGLE_SHIFT, 4) as f64 * -std::f64::consts::TAU / 16.0,
        // Terrain3D's DECODE_SCALE, so the unmodified 0 halves the uv and the others step from there.
        scale: 0.9 - (((field(SCALE_SHIFT, 3) + 3) % 8) + 1) as f64 * 0.1,
        hole: field(HOLE_BIT, 1) == 1,
        auto: field(AUTO_BIT, 1) == 1,
    }
}

fn rotate(v: DVec2, angle: f64) -> DVec2 {
    let (s, c) = angle.sin_cos();
    DVec2::new(c * v.x + s * v.y, c * v.y - s * v.x)
}

fn random(xy: DVec2) -> f64 {
    (xy.dot(DVec2::new(12.9898, 78.233)).sin() * 43758.5453).rem_euclid(1.0)
}

/// Where Terrain3D samples a texture of `uv_scale` and `detiling` (rotation and shift) at `world` meters, in a cell
/// whose corner vertex sits at `corner` meters and holds `control`. The twin of `texture_uv` in clipmap.wgsl.
pub fn texture_uv(world: DVec2, corner: DVec2, control: u32, uv_scale: f64, detiling: [f64; 2]) -> DVec2 {
    let d = decode(control);
    let uv = rotate(world * d.scale, d.angle);
    let pos = rotate(corner * d.scale, d.angle);
    let center = (pos * uv_scale + 0.5).floor();
    let tile = (random(center) * 2.0 - 1.0) * std::f64::consts::TAU;
    rotate(uv * uv_scale - center, tile * detiling[0]) + center + tile * detiling[1] - 0.5
}

/// The image a texture draws with.
#[derive(Clone, Debug)]
pub enum Albedo {
    /// The albedo Godot exported at [`ALBEDO_SIDE`].
    Full(image::RgbaImage),
    /// The small preview Godot saves, standing in when the full albedo is missing.
    Thumbnail(image::RgbaImage),
    /// No image, the texture draws its color alone.
    None,
}

/// One Terrain3D texture asset.
#[derive(Clone, Debug)]
pub struct ClipmapTexture {
    pub id: u32,
    pub albedo: Albedo,
    /// Terrain3D's `albedo_color`, multiplied with the albedo as its shader does.
    pub color: [f32; 3],
    pub uv_scale: f32,
    /// Terrain3D's `detiling_rotation` and `detiling_shift`.
    pub detiling: [f32; 2],
}

/// Side of the albedo array: full albedos when every texture has one, else thumbnails, else 1 pixel swatches.
pub fn albedo_side(textures: &[ClipmapTexture]) -> u32 {
    side_of(textures.iter())
}

fn side_of<'a>(textures: impl Iterator<Item = &'a ClipmapTexture> + Clone) -> u32 {
    if textures.clone().all(|t| matches!(t.albedo, Albedo::None)) {
        1
    } else if textures.clone().all(|t| matches!(t.albedo, Albedo::Full(_))) {
        ALBEDO_SIDE
    } else {
        THUMBNAIL_SIDE
    }
}

/// One texture in the clipmap uniform, see clipmap.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct TextureUniform {
    pub(crate) uv: [f32; 4],
    pub(crate) color: [f32; 4],
    pub(crate) mean: [f32; 4],
}

impl Default for TextureUniform {
    fn default() -> Self {
        Self { uv: [DEFAULT_UV_SCALE, 0.0, 0.0, 0.0], color: UNLISTED_COLOR, mean: [1.0; 4] }
    }
}

/// The albedo array's layers by texture id, white where a texture has no image, and the uniform entries.
pub(crate) struct AlbedoLayers {
    pub side: u32,
    pub layers: Vec<image::RgbaImage>,
    pub entries: [TextureUniform; MAX_TEXTURES],
}

impl AlbedoLayers {
    /// Ids past the last texture id are left out. Of two textures with one id the first counts.
    pub fn new(textures: &[ClipmapTexture]) -> Self {
        let textures: Vec<&ClipmapTexture> = textures
            .iter()
            .enumerate()
            .filter(|&(i, t)| (t.id as usize) < MAX_TEXTURES && !textures[..i].iter().any(|o| o.id == t.id))
            .map(|(_, t)| t)
            .collect();
        let side = side_of(textures.iter().copied());
        let count = textures.iter().map(|t| t.id as usize + 1).max().unwrap_or(1);
        let white = image::RgbaImage::from_pixel(side, side, image::Rgba([255; 4]));
        let mut layers = vec![white; count];
        let mut entries = [TextureUniform::default(); MAX_TEXTURES];
        for t in textures {
            let mut mean = [1.0; 3];
            if let Albedo::Full(img) | Albedo::Thumbnail(img) = &t.albedo {
                let mut layer = if img.dimensions() == (side, side) {
                    img.clone()
                } else {
                    image::imageops::resize(img, side, side, image::imageops::FilterType::Triangle)
                };
                // Terrain3D keeps the height in the alpha channel.
                layer.pixels_mut().for_each(|p| p.0[3] = 255);
                mean = linear_mean(&layer);
                layers[t.id as usize] = layer;
            }

            entries[t.id as usize] = TextureUniform {
                uv: [t.uv_scale, t.detiling[0], t.detiling[1], 0.0],
                color: [t.color[0], t.color[1], t.color[2], 1.0],
                mean: [mean[0], mean[1], mean[2], 1.0],
            };
        }

        Self { side, layers, entries }
    }
}

fn linear_mean(img: &image::RgbaImage) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    for p in img.pixels() {
        for (s, v) in sum.iter_mut().zip(p.0) {
            *s += crate::srgb_to_linear(v as f32 / 255.0) as f64;
        }
    }

    let n = (img.width() as f64 * img.height() as f64).max(1.0);
    sum.map(|s| (s / n) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(id: u32, albedo: Albedo) -> ClipmapTexture {
        ClipmapTexture { id, albedo, color: [0.9, 0.8, 0.7], uv_scale: 0.25, detiling: [0.5, 0.125] }
    }

    fn solid(side: u32, rgba: [u8; 4]) -> image::RgbaImage {
        image::RgbaImage::from_pixel(side, side, image::Rgba(rgba))
    }

    #[test]
    fn the_albedo_array_side_follows_the_poorest_texture() {
        let full = || Albedo::Full(solid(512, [10, 20, 30, 255]));
        let thumb = || Albedo::Thumbnail(solid(64, [10, 20, 30, 255]));
        let cases = [
            (vec![], 1),
            (vec![texture(0, Albedo::None)], 1),
            (vec![texture(0, Albedo::None), texture(3, Albedo::None)], 1),
            (vec![texture(2, full())], 512),
            (vec![texture(0, full()), texture(5, full()), texture(31, full())], 512),
            (vec![texture(0, full()), texture(1, thumb())], 64),
            (vec![texture(0, full()), texture(1, Albedo::None)], 64),
            (vec![texture(4, thumb())], 64),
            (vec![texture(0, Albedo::None), texture(1, thumb()), texture(2, full())], 64),
        ];
        for (textures, want) in cases {
            let kinds: Vec<_> = textures.iter().map(|t| (t.id, std::mem::discriminant(&t.albedo))).collect();
            assert_eq!(albedo_side(&textures), want, "{kinds:?}");
        }
    }

    #[test]
    fn layers_sit_at_their_id_with_alpha_dropped() {
        let red = solid(512, [255, 0, 0, 40]);
        let mut half = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        (0..32).for_each(|x| (0..64).for_each(|z| half.put_pixel(x, z, image::Rgba([255, 255, 255, 3]))));
        let textures = [
            texture(3, Albedo::Full(red)),
            texture(1, Albedo::Thumbnail(half)),
            ClipmapTexture { color: [0.1, 0.2, 0.3], ..texture(5, Albedo::None) },
            ClipmapTexture { color: [0.0; 3], ..texture(3, Albedo::None) },
            texture(32, Albedo::None),
        ];
        let got = AlbedoLayers::new(&textures);
        assert_eq!((got.side, got.layers.len()), (64, 6), "thumbnails set the side, the last id the count");
        assert!(got.layers.iter().all(|l| l.dimensions() == (64, 64) && l.pixels().all(|p| p.0[3] == 255)));
        assert_eq!(got.layers[3].get_pixel(17, 40).0, [255, 0, 0, 255], "the full albedo is resized to the side");
        assert_eq!([got.layers[1].get_pixel(0, 9).0, got.layers[1].get_pixel(63, 9).0], [[255; 4], [0, 0, 0, 255]]);
        for id in [0, 2, 4, 5] {
            assert!(got.layers[id].pixels().all(|p| p.0 == [255; 4]), "layer {id} without an image is white");
        }

        let entry = |uv: [f32; 4], color: [f32; 3], mean: [f32; 3]| TextureUniform {
            uv,
            color: [color[0], color[1], color[2], 1.0],
            mean: [mean[0], mean[1], mean[2], 1.0],
        };
        let mut want = [TextureUniform::default(); MAX_TEXTURES];
        want[3] = entry([0.25, 0.5, 0.125, 0.0], [0.9, 0.8, 0.7], [1.0, 0.0, 0.0]);
        want[1] = entry([0.25, 0.5, 0.125, 0.0], [0.9, 0.8, 0.7], [0.5, 0.5, 0.5]);
        want[5] = entry([0.25, 0.5, 0.125, 0.0], [0.1, 0.2, 0.3], [1.0; 3]);
        assert_eq!(got.entries, want, "the first of two textures with an id counts, ids past 31 are dropped");
        assert_eq!(TextureUniform::default().color, [0.5, 0.5, 0.5, 1.0]);

        let none = AlbedoLayers::new(&[]);
        assert_eq!((none.side, none.layers.len(), none.layers[0].get_pixel(0, 0).0), (1, 1, [255; 4]));
        assert_eq!(none.entries, [TextureUniform::default(); MAX_TEXTURES]);
    }

    #[test]
    fn decoding_scales_follow_terrain3d() {
        // Terrain3DUtil documents the steps as 0, 20, 40, 60, 80, -60, -40 and -20 percent of texture size.
        let scales = (0..8).map(|bits| decode(bits << 7).scale).collect::<Vec<_>>();
        let want = [0.5, 0.4, 0.3, 0.2, 0.1, 0.8, 0.7, 0.6];
        for (got, want) in scales.iter().zip(want) {
            assert!((got - want).abs() < 1e-12, "{scales:?}");
        }

        let d = decode((31 << 27) | (3 << 22) | (128 << 14) | (4 << 10) | (3 << 7) | (1 << 2) | 1);
        assert_eq!((d.base, d.overlay, d.blend, d.hole, d.auto), (31, 3, 128.0 / 255.0, true, true));
        assert!((d.angle + std::f64::consts::FRAC_PI_2).abs() < 1e-12 && (d.scale - 0.2).abs() < 1e-12);
    }

    #[test]
    fn texture_uvs_match_terrain3d() {
        let close = |got: DVec2, want: [f64; 2]| (got - DVec2::from(want)).abs().max_element() < 1e-9;
        let at = DVec2::new(13.0, -7.0);
        let cases = [
            // The unmodified control halves the uv, then Terrain3D shifts every texture by -0.5.
            (at, 0, 0.1, [0.0, 0.0], [0.15, -0.85]),
            // Scale step 5 (0.8) and angle step 4, a quarter turn: (x, y) becomes (-y, x).
            (at, (5 << 7) | (4 << 10), 0.25, [0.0, 0.0], [0.9, 2.1]),
            // Angle step 2 turns by -45 degrees: (x, y) becomes ((x - y) / sqrt 2, (x + y) / sqrt 2).
            (at, 2 << 10, 0.1, [0.0, 0.0], [20.0 * 0.5 * 0.1 / 2f64.sqrt() - 0.5, 6.0 * 0.5 * 0.1 / 2f64.sqrt() - 0.5]),
        ];
        for (world, control, scale, detiling, want) in cases {
            let got = texture_uv(world, DVec2::new(-40.0, 25.0), control, scale, detiling);
            assert!(close(got, want), "{control:b}: {got} != {want:?}");
        }

        // Near the origin the tile center is (0, 0), whose random value is 0, so the detiling turns by -rotation and
        // shifts by -shift turns: a quarter turn of (0.3, 0.2) is (-0.2, 0.3).
        let got = texture_uv(DVec2::new(3.0, 2.0), DVec2::new(2.0, 1.0), 0, 0.2, [0.25, 0.1]);
        let shift = -0.1 * std::f64::consts::TAU - 0.5;
        assert!(close(got, [-0.2 + shift, 0.3 + shift]), "{got}");
        let far = texture_uv(DVec2::new(203.0, 2.0), DVec2::new(202.0, 1.0), 0, 0.2, [0.0, 0.0]);
        assert!(close(far, [20.3 - 0.5, 0.2 - 0.5]), "no detiling leaves every tile in place: {far}");
    }
}
