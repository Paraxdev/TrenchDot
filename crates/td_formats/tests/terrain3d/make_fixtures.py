"""Writes the Terrain3D cache fixtures the way the Godot addon lays them out, independent of the Rust writer.

Needs Python 3.14 for compression.zstd. The patterns are repeated in tests/terrain3d_cache.rs and
godot/tests/terrain3d_export_tests.gd, change all three together. A region hash is the sha256 of the uncompressed
maps, as the addon computes it.
"""

import hashlib
import json
import struct
from compression import zstd
from pathlib import Path

SIZE = 64
SPACING = 2.5
CACHE = Path(__file__).parent / "cache"


def stem(loc):
    part = lambda v: f"{v:03d}" if v < 0 else f"_{v:02d}"
    return f"terrain3d{part(loc[0])}{part(loc[1])}"


def region_hash(loc):
    return hashlib.sha256(maps(loc, 0.0, loc == (-1, 0))).digest()


def maps(loc, lift, color):
    lx, lz = loc
    heights, control, rgba = bytearray(), bytearray(), bytearray()
    for z in range(SIZE):
        for x in range(SIZE):
            i = z * SIZE + x
            heights += struct.pack("<f", lx * 8 + lz * 0.5 + x * 0.25 - z * 0.125 + lift)
            c = (i * 2654435761 + (lx % 2**32) * 40503 + (lz % 2**32) * 977) % 2**32
            c = {0: 0x7FC00001, 1: 0xFF800001}.get(i, c)
            control += struct.pack("<I", c)
            if color:
                rgba += bytes([x * 4, z * 4, (x + z) % 256, 255 - x])
    return bytes(heights + control + rgba)


def td3r(loc, header_hash, lift, color):
    head = b"TD3R" + struct.pack("<IiiIfI", 1, loc[0], loc[1], SIZE, SPACING, 1 if color else 0) + header_hash
    return head + zstd.compress(maps(loc, lift, color))


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


write(CACHE / "regions" / f"{stem((-1, 0))}.td3r", td3r((-1, 0), region_hash((-1, 0)), 0.0, True))
write(CACHE / "regions" / f"{stem((3, -2))}.td3r", td3r((3, -2), region_hash((3, -2)), 0.0, False))
pending = td3r((3, -2), bytes(32), 1.0, False) + b"BASE" + region_hash((3, -2)) + struct.pack("<I", 1)
write(CACHE / "pending" / f"{stem((3, -2))}.td3r", pending)

manifest = {
    "format": "trenchdot-terrain3d",
    "version": 1,
    "terrain3d": "1.1.0",
    "godot": "4.7.2",
    "data_version": 1.1,
    "region_size": SIZE,
    "vertex_spacing": SPACING,
    "material": {"world_background": 1, "ground_level": -2.0, "region_blend": 0.25, "auto_shader": True, "dual_scaling": True},
    "map_transform_identity": False,
    "editor_open": True,
    "regions": [
        {"loc": [-1, 0], "hash": region_hash((-1, 0)).hex(), "height_range": [-15.875, 7.75], "instances": 12},
        {"loc": [3, -2], "hash": region_hash((3, -2)).hex(), "height_range": [15.125, 38.75]},
    ],
    "dirty": [[3, -2]],
    "textures": [
        {"id": 0, "name": "Grass", "albedo_color": [1, 1, 1, 1], "thumbnail": "textures/0.png"},
        {"id": 5, "name": "Rock", "albedo_color": [0.5, 0.25, 0.125, 1], "thumbnail": ""},
    ],
    "applied": [{"seq": 7, "accepted": [[-1, 0]], "refused": [{"loc": [3, -2], "reason": "dirty"}]}],
    "future_key": {"anything": [1, 2]},
}
write(CACHE / "manifest.json", (json.dumps(manifest, indent=2) + "\n").encode())
