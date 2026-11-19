# Terrain3D cache

TrenchDot never writes a Terrain3D `.res` file. Godot does that through Terrain3D's own API, and the two programs
trade data through a cache folder next to the regions. TrenchDot only reads the `.res` files of a folder that has no
cache yet, and writes nothing for it. This page is for tools that want to read or produce it.
To use Terrain3D, read [Terrain3D](../godot/terrain3d.md) instead.

The cache is `<data folder>/.trenchdot/`, one per data folder, shared by every map that links it. It is safe to delete,
Godot writes it again the next time it opens the scene or runs the [command line](../godot/terrain3d.md#without-the-godot-editor-open).
Godot's FileSystem dock and exports skip it, and Terrain3D only reads `terrain3d*.res`, so nothing in it ends up in a game.

| Path | Written by | What it is |
| --- | --- | --- |
| `manifest.json` | Godot | The settings, region list, hashes and texture list |
| `regions/terrain3d_XX_YY.td3r` | Godot | What Terrain3D holds in memory for each region |
| `textures/<id>.png` | Godot | A thumbnail of each Terrain3D texture asset |
| `textures/<id>_albedo.png` | Godot | The albedo of each texture asset at 512 px, for drawing the terrain |
| `pending/terrain3d_XX_YY.td3r` | TrenchDot | A region edit waiting for Godot, deleted once Godot applied or refused it |
| `lod/terrain3d_XX_YY.<hash>.td3l` | TrenchDot | Coarser copies of a region for the 3D view, safe to delete |
| `backups/<time>/` | Godot | Copies of the region files and `manifest.json` from before the command line changed the region size or spacing, named after the UTC time like `20261119-101500`. The last three are kept |

Every writer writes a temporary file and renames it, so a reader never sees half a file. Names follow Terrain3D: a
negative coordinate is written `-01` and a positive one `_02`, so region (-1, 2) is `terrain3d-01_02`. Regions run from
-16 to 15 on each axis.

## manifest.json

```json
{
  "format": "trenchdot-terrain3d",
  "version": 1,
  "terrain3d": "1.1.0",
  "godot": "4.7.2",
  "data_version": 1.1,
  "region_size": 256,
  "vertex_spacing": 1.0,
  "material": {
    "world_background": 2, "ground_level": 0.0, "region_blend": 0.0, "auto_shader": true,
    "world_noise": { "scale": 5.0, "height": 64.0, "offset": [0, 0, 0], "lod_distance": 7500.0, "max_octaves": 4, "min_octaves": 2, "fragment_normals": false },
    "auto": { "slope": 1.0, "height_reduction": 0.1, "base_texture": 0, "overlay_texture": 1 },
    "blend_sharpness": 0.5, "dual_scaling": true, "macro_variation": true, "projection": true
  },
  "map_transform_identity": true,
  "editor_open": false,
  "regions": [ { "loc": [-1, 0], "hash": "a2642a6b...", "height_range": [-6.89, 29.4] } ],
  "dirty": [],
  "textures": [ {
    "id": 0, "name": "Grass", "albedo_color": [1, 1, 1, 1], "thumbnail": "textures/0.png",
    "uv_scale": 0.1, "detiling_rotation": 0.0, "detiling_shift": 0.0, "albedo": "textures/0_albedo.png", "hash": "9a0364b9..."
  } ],
  "ocean": { "enabled": true, "sea_level": 5.0, "water_color": [0.1058, 0.1568, 0.2117, 1], "source": "res://water/M_ocean.tres" },
  "resources": { "material": "res://data/M_terrain.tres", "assets": "res://data/assets.tres", "ocean_material": "res://water/M_ocean.tres" },
  "applied": [ { "seq": 7, "accepted": [[-1, 0]], "refused": [ { "loc": [0, 0], "reason": "dirty" } ] } ]
}
```

| Key | Meaning |
| --- | --- |
| `format`, `version` | Always `trenchdot-terrain3d` and `1`. Another format or a newer version is refused |
| `terrain3d`, `godot`, `data_version` | The versions that wrote the cache, TrenchDot warns about a Terrain3D it has not been tested with |
| `region_size`, `vertex_spacing` | Vertices per region side and meters between vertices |
| `material` | The Terrain3D material settings: those the surface height depends on near region edges, the world background beyond the regions (`world_background` 0 for none, 1 flat, 2 noise shaped by `world_noise`), the auto shader and texture blending |
| `map_transform_identity` | `false` when the map node in the scene is not at the origin, Terrain3D ignores that transform |
| `editor_open` | Whether a Godot editor had the scene open when this was written |
| `regions` | One entry per region: `loc`, `hash` (sha256 as hex) and `height_range` in meters |
| `dirty` | Regions with edits the Godot editor has not saved yet |
| `textures` | One entry per texture asset: its `id`, `name`, `albedo_color`, uv scale and detiling, the paths of its `thumbnail` and full `albedo` relative to the cache folder, and a `hash` that changes with the image or the uv values |
| `applied` | What Godot did with the last pending files, `reason` is `dirty`, `changed` or `size` |
| `ocean` | The Terrain3D ocean: `enabled`, `sea_level` in meters, `water_color` and the res path of its material in `source` |
| `resources` | The res paths of the node's `material`, `assets` and `ocean_material`, empty for none or a built in one |
| `scene_vertex_spacing` | The vertex spacing the command line set with Godot closed, which the scene's node takes the next time the Godot editor opens it. It stays until the scene is saved there |

Unknown keys are ignored, and only `format`, `version`, `region_size` and `vertex_spacing` are required, so caches
written by an older addon still read. A missing key means:

| Key | When missing |
| --- | --- |
| `material.world_noise` | No noise values are known. A key missing inside it takes the Terrain3D shader default: `scale` 5, `height` 32, `offset` 0, `lod_distance` 7500, `max_octaves` 4, `min_octaves` 2 |
| `material.auto` | The Terrain3D shader defaults: `slope` 1, `height_reduction` 0.1, `base_texture` 0, `overlay_texture` 1 |
| `material.blend_sharpness` | 0.5 |
| `textures[].uv_scale`, `detiling_rotation`, `detiling_shift` | 0.1, 0 and 0 |
| `textures[].albedo` | No full size image, the thumbnail stands in |
| `ocean` | No water |
| `resources` | Unknown, the addon is older than the key |

## Region files

Little endian. The header is `TD3R`, a `u32` version (1), `i32` x and y, `u32` size, `f32` vertex spacing, a `u32` flags
word (bit 0 means a color map follows) and the 32 byte region hash. Then comes one zstd frame holding:

1. `size * size` `f32` heights in meters, row by row from north to south, west to east inside a row.
2. `size * size` `u32` control words, the raw bits as Terrain3D stores them, NaN patterns included.
3. When flagged, `size * size * 4` bytes of RGBA8 color, where alpha is roughness.

The hash is computed by Godot over the height, control and color bytes. TrenchDot compares it and echoes it back, and
computes it the same way for a folder it reads from the `.res` files, so a cache written later matches.

A pending file has the same layout plus a 40 byte trailer: `BASE`, the 32 byte hash of the region the edit started from
(all zero for a new region) and a `u32` flags word, bit 0 for force and bit 1 for removing the region.

## How an edit is applied

Godot applies a pending region only when the region still has the hash in the trailer and is not in `dirty`. A region
that fails that is refused with `changed` or `dirty` and left alone, the others still apply. A pending file with the
force flag skips both checks. Godot then deletes the pending file and lists the result under `applied`.
