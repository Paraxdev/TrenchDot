# Terrain3D cache

TrenchDot never reads or writes a Terrain3D `.res` file. Godot does that through Terrain3D's own API, and the two
programs trade data through a cache folder next to the regions. This page is for tools that want to read or produce it.
To use Terrain3D, read [Terrain3D](../godot/terrain3d.md) instead.

The cache is `<data folder>/.trenchdot/`, one per data folder, shared by every map that links it. It is safe to delete,
Godot writes it again the next time it opens the scene or runs the [command line](../godot/terrain3d.md#without-the-godot-editor-open).
Godot's FileSystem dock and exports skip it, and Terrain3D only reads `terrain3d*.res`, so nothing in it ends up in a game.

| Path | Written by | What it is |
| --- | --- | --- |
| `manifest.json` | Godot | The settings, region list, hashes and texture list |
| `regions/terrain3d_XX_YY.td3r` | Godot | What Terrain3D holds in memory for each region |
| `textures/<id>.png` | Godot | A thumbnail of each Terrain3D texture asset |
| `pending/terrain3d_XX_YY.td3r` | TrenchDot | A region edit waiting for Godot, deleted once Godot applied or refused it |
| `lod/terrain3d_XX_YY.<hash>.td3l` | TrenchDot | Coarser copies of a region for the 3D view, safe to delete |

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
  "material": { "world_background": 0, "ground_level": 0.0, "region_blend": 0.0, "auto_shader": false },
  "map_transform_identity": true,
  "editor_open": false,
  "regions": [ { "loc": [-1, 0], "hash": "a2642a6b...", "height_range": [-6.89, 29.4] } ],
  "dirty": [],
  "textures": [ { "id": 0, "name": "Grass", "albedo_color": [1, 1, 1, 1], "thumbnail": "textures/0.png" } ],
  "applied": [ { "seq": 7, "accepted": [[-1, 0]], "refused": [ { "loc": [0, 0], "reason": "dirty" } ] } ]
}
```

| Key | Meaning |
| --- | --- |
| `format`, `version` | Always `trenchdot-terrain3d` and `1`. Another format or a newer version is refused |
| `terrain3d`, `godot`, `data_version` | The versions that wrote the cache, TrenchDot warns about a Terrain3D it has not been tested with |
| `region_size`, `vertex_spacing` | Vertices per region side and meters between vertices |
| `material` | The Terrain3D material settings the surface height depends on near region edges |
| `map_transform_identity` | `false` when the map node in the scene is not at the origin, Terrain3D ignores that transform |
| `editor_open` | Whether a Godot editor had the scene open when this was written |
| `regions` | One entry per region: `loc`, `hash` (sha256 as hex) and `height_range` in meters |
| `dirty` | Regions with edits the Godot editor has not saved yet |
| `applied` | What Godot did with the last pending files, `reason` is `dirty`, `changed` or `size` |

Unknown keys are ignored.

## Region files

Little endian. The header is `TD3R`, a `u32` version (1), `i32` x and y, `u32` size, `f32` vertex spacing, a `u32` flags
word (bit 0 means a color map follows) and the 32 byte region hash. Then comes one zstd frame holding:

1. `size * size` `f32` heights in meters, row by row from north to south, west to east inside a row.
2. `size * size` `u32` control words, the raw bits as Terrain3D stores them, NaN patterns included.
3. When flagged, `size * size * 4` bytes of RGBA8 color, where alpha is roughness.

The hash is computed by Godot over the height, control and color bytes. TrenchDot only compares it and echoes it back,
it never computes one.

A pending file has the same layout plus a 40 byte trailer: `BASE`, the 32 byte hash of the region the edit started from
(all zero for a new region) and a `u32` flags word, bit 0 for force and bit 1 for removing the region.

## How an edit is applied

Godot applies a pending region only when the region still has the hash in the trailer and is not in `dirty`. A region
that fails that is refused with `changed` or `dirty` and left alone, the others still apply. A pending file with the
force flag skips both checks. Godot then deletes the pending file and lists the result under `applied`.
