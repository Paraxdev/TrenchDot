#!/usr/bin/env python3
"""Fetches the Terrain3D GDExtension into godot/addons/terrain_3d for the Terrain3D bridge and its tests.

  python tools/fetch_terrain3d.py                # download the pinned release once, extract addons/terrain_3d
  python tools/fetch_terrain3d.py --check        # verify the cached zip and the extracted files, no network
  python tools/fetch_terrain3d.py --zip FILE     # use a release zip on disk instead of downloading
  python tools/fetch_terrain3d.py --dest DIR     # extract somewhere else, for example a throwaway Godot project

Terrain3D is MIT licensed and about 85 MB of binaries for every platform, so it is fetched, never committed, and
the addon and demo zips leave it out. The download is cached in target/terrain3d. Bumping the pin means changing
VERSION, SIZE and SHA256 together.
"""
import argparse
import hashlib
import pathlib
import shutil
import sys
import urllib.request
import zipfile
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = "1.1.0-stable"
URL = f"https://github.com/TokisanGames/Terrain3D/releases/download/v{VERSION}/Terrain3D_v{VERSION}.zip"
SIZE = 45665295
SHA256 = "51b78bc758f5759cbc8eaaab14e5cf4e8661717e265fdd760f5abf945f15b243"
CACHE = ROOT / "target" / "terrain3d" / f"Terrain3D_v{VERSION}.zip"
DEST = ROOT / "godot" / "addons" / "terrain_3d"
PREFIX = "addons/terrain_3d/"
STAMP = ".fetched"


def zip_problem(path):
    """None when the file is the pinned release, else why not."""
    if not path.is_file():
        return f"{path} is missing"
    size = path.stat().st_size
    if size != SIZE:
        return f"{path} is {size} bytes, expected {SIZE}"
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    if digest.hexdigest() != SHA256:
        return f"{path} has sha256 {digest.hexdigest()}, expected {SHA256}"
    return None


def addon_entries(archive):
    """(relative path, ZipInfo) for every file under addons/terrain_3d, refusing paths that leave it."""
    entries = []
    for info in archive.infolist():
        if not info.filename.startswith(PREFIX) or info.is_dir():
            continue
        rel = pathlib.PurePosixPath(info.filename[len(PREFIX):])
        if rel.is_absolute() or ".." in rel.parts or ":" in info.filename:
            raise ValueError(f"unsafe path in the zip: {info.filename}")
        entries.append((rel, info))
    if not entries:
        raise ValueError(f"the zip has no {PREFIX}")
    return entries


def download(dest):
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(".part")
    print(f"downloading {URL}")
    with urllib.request.urlopen(URL) as response, part.open("wb") as out:
        shutil.copyfileobj(response, out)
    problem = zip_problem(part)
    if problem:
        part.unlink()
        raise SystemExit(f"download rejected, {problem}")
    part.replace(dest)


def extract(zip_path, dest):
    if dest.exists():
        if not (dest / STAMP).is_file() and any(dest.iterdir()):
            raise SystemExit(f"{dest} exists and was not made by this script, move it away first")
        # Replaced whole so a version bump leaves no stale binaries behind.
        shutil.rmtree(dest)
    with zipfile.ZipFile(zip_path) as archive:
        for rel, info in addon_entries(archive):
            target = dest.joinpath(*rel.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(info) as src, target.open("wb") as out:
                shutil.copyfileobj(src, out)
    (dest / STAMP).write_text(f"{VERSION}\n{SHA256}\n", encoding="utf-8")
    print(f"Terrain3D {VERSION} extracted into {dest}")


def check(zip_path, dest):
    """Problems with the cached zip or the files extracted from it. Files Godot adds (.import, .uid) are fine."""
    problem = zip_problem(zip_path)
    if problem:
        return [problem]
    stamp = dest / STAMP
    if not stamp.is_file():
        return [f"{dest} was not extracted by this script"]
    if stamp.read_text(encoding="utf-8").split() != [VERSION, SHA256]:
        return [f"{dest} holds another Terrain3D than {VERSION}"]
    problems = []
    with zipfile.ZipFile(zip_path) as archive:
        for rel, info in addon_entries(archive):
            target = dest.joinpath(*rel.parts)
            if not target.is_file():
                problems.append(f"{target} is missing")
            elif target.stat().st_size != info.file_size or zlib.crc32(target.read_bytes()) != info.CRC:
                problems.append(f"{target} differs from the release")
    return problems


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="verify the zip and the extracted files, no network")
    parser.add_argument("--zip", type=pathlib.Path, help=f"release zip to use instead of {CACHE.relative_to(ROOT)}")
    parser.add_argument("--dest", type=pathlib.Path, default=DEST, help="folder to extract addons/terrain_3d into")
    args = parser.parse_args(argv)
    zip_path = args.zip or CACHE
    dest = args.dest.resolve()

    if args.check:
        problems = check(zip_path, dest)
        for line in problems:
            print(line, file=sys.stderr)
        if problems:
            print(f"run python tools/fetch_terrain3d.py to fetch Terrain3D {VERSION} again", file=sys.stderr)
            return 1
        print(f"Terrain3D {VERSION} in {dest} matches the pinned release")
        return 0

    problem = zip_problem(zip_path)
    if problem and args.zip:
        raise SystemExit(problem)
    if problem:
        download(zip_path)
    extract(zip_path, dest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
