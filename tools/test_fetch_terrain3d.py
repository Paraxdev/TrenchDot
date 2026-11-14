#!/usr/bin/env python3
"""Tests of tools/fetch_terrain3d.py, `python3 tools/test_fetch_terrain3d.py`. They build a small stand in for the
release zip, so they need no network. The last test checks the real cached zip when one has been fetched."""
import contextlib
import hashlib
import io
import pathlib
import shutil
import sys
import tempfile
import unittest
import zipfile
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import fetch_terrain3d as ft  # noqa: E402

FILES = {
    "addons/terrain_3d/terrain.gdextension": b"[configuration]\nentry_symbol = \"terrain_3d_init\"\n",
    "addons/terrain_3d/bin/libterrain.windows.release.x86_64.dll": bytes(range(256)) * 7,
    "addons/terrain_3d/src/editor.gd": b"@tool\nextends EditorPlugin\n",
    "demo/Demo.tscn": b"[gd_scene format=3]\n",
}


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.zip = self.make_zip(self.tmp / "t3d.zip", FILES)
        data = self.zip.read_bytes()
        pin = mock.patch.multiple(ft, SIZE=len(data), SHA256=hashlib.sha256(data).hexdigest())
        pin.start()
        self.addCleanup(pin.stop)
        self.dest = self.tmp / "project" / "addons" / "terrain_3d"

    def make_zip(self, path, files):
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in files.items():
                archive.writestr(name, data)
        return path

    def run_main(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = ft.main([*args, "--zip", str(self.zip), "--dest", str(self.dest)])
        return code, out.getvalue(), err.getvalue()

    def test_pinned_zip_passes(self):
        self.assertIsNone(ft.zip_problem(self.zip))

    def test_tampered_byte_fails_on_the_hash(self):
        data = bytearray(self.zip.read_bytes())
        data[len(data) // 2] ^= 0x01
        self.zip.write_bytes(bytes(data))
        self.assertIn("has sha256", ft.zip_problem(self.zip))

    def test_truncated_zip_fails_on_the_size(self):
        self.zip.write_bytes(self.zip.read_bytes()[:-1])
        problem = ft.zip_problem(self.zip)
        self.assertIn(f"expected {ft.SIZE}", problem)
        self.assertNotIn("sha256", problem)

    def test_missing_zip_fails(self):
        self.assertIn("is missing", ft.zip_problem(self.tmp / "none.zip"))

    def test_extracts_only_the_addon(self):
        code, out, _ = self.run_main()
        self.assertEqual(code, 0)
        files = sorted(p.relative_to(self.dest).as_posix() for p in self.dest.rglob("*") if p.is_file())
        self.assertEqual(files, [".fetched", "bin/libterrain.windows.release.x86_64.dll", "src/editor.gd", "terrain.gdextension"])
        self.assertEqual((self.dest / "src" / "editor.gd").read_bytes(), FILES["addons/terrain_3d/src/editor.gd"])
        self.assertFalse((self.dest.parent.parent / "demo").exists())
        self.assertIn(ft.VERSION, out)

    def test_check_passes_after_extract_and_ignores_files_godot_adds(self):
        self.run_main()
        (self.dest / "src" / "editor.gd.uid").write_text("uid://b1\n")
        code, out, err = self.run_main("--check")
        self.assertEqual((code, err), (0, ""))
        self.assertIn("matches the pinned release", out)

    def test_check_fails_on_a_tampered_extracted_file_of_the_same_size(self):
        self.run_main()
        dll = self.dest / "bin" / "libterrain.windows.release.x86_64.dll"
        data = bytearray(dll.read_bytes())
        data[0] ^= 0xFF
        dll.write_bytes(bytes(data))
        code, _, err = self.run_main("--check")
        self.assertEqual(code, 1)
        self.assertIn("libterrain.windows.release.x86_64.dll differs", err)
        self.assertNotIn("editor.gd", err)

    def test_check_fails_on_a_missing_extracted_file(self):
        self.run_main()
        (self.dest / "terrain.gdextension").unlink()
        code, _, err = self.run_main("--check")
        self.assertEqual(code, 1)
        self.assertIn("terrain.gdextension is missing", err)

    def test_check_fails_on_a_tampered_zip(self):
        self.run_main()
        self.zip.write_bytes(self.zip.read_bytes() + b"\0")
        code, _, err = self.run_main("--check")
        self.assertEqual(code, 1)
        self.assertIn(f"expected {ft.SIZE}", err)

    def test_check_fails_without_the_stamp_or_with_another_version(self):
        self.dest.mkdir(parents=True)
        code, _, err = self.run_main("--check")
        self.assertEqual(code, 1)
        self.assertIn("was not extracted by this script", err)
        self.run_main()
        (self.dest / ft.STAMP).write_text(f"1.0.2-stable\n{ft.SHA256}\n")
        code, _, err = self.run_main("--check")
        self.assertEqual(code, 1)
        self.assertIn(f"another Terrain3D than {ft.VERSION}", err)

    def test_refetch_replaces_its_own_folder_and_drops_stale_files(self):
        self.run_main()
        stale = self.dest / "bin" / "libterrain.old.dll"
        stale.write_bytes(b"old")
        self.assertEqual(self.run_main()[0], 0)
        self.assertFalse(stale.exists())
        self.assertTrue((self.dest / "terrain.gdextension").is_file())

    def test_refuses_to_replace_a_folder_it_did_not_make(self):
        self.dest.mkdir(parents=True)
        own = self.dest / "mine.gd"
        own.write_text("extends Node\n")
        with self.assertRaises(SystemExit) as raised:
            self.run_main()
        self.assertIn("was not made by this script", str(raised.exception))
        self.assertEqual(own.read_text(), "extends Node\n")

    def test_an_empty_folder_is_filled(self):
        self.dest.mkdir(parents=True)
        self.assertEqual(self.run_main()[0], 0)
        self.assertTrue((self.dest / ft.STAMP).is_file())

    def test_refuses_paths_that_leave_the_addon(self):
        for name in ("addons/terrain_3d/../../evil.gd", "addons/terrain_3d/C:/evil.gd"):
            with self.subTest(name=name):
                bad = self.make_zip(self.tmp / "bad.zip", {**FILES, name: b"x"})
                with zipfile.ZipFile(bad) as archive, self.assertRaises(ValueError) as raised:
                    ft.addon_entries(archive)
                self.assertIn("unsafe path", str(raised.exception))

    def test_refuses_a_zip_without_the_addon(self):
        bad = self.make_zip(self.tmp / "bad.zip", {"demo/Demo.tscn": b""})
        with zipfile.ZipFile(bad) as archive, self.assertRaises(ValueError) as raised:
            ft.addon_entries(archive)
        self.assertIn("has no addons/terrain_3d/", str(raised.exception))

    def test_a_given_zip_that_is_not_the_pin_stops_before_extracting(self):
        self.zip.write_bytes(self.zip.read_bytes()[:-1])
        with self.assertRaises(SystemExit) as raised:
            self.run_main()
        self.assertIn("bytes, expected", str(raised.exception))
        self.assertFalse(self.dest.exists())

    def test_a_rejected_download_leaves_no_file_behind(self):
        target = self.tmp / "cache" / "t3d.zip"
        with mock.patch.object(ft.urllib.request, "urlopen", return_value=io.BytesIO(b"not the release")), \
                contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as raised:
            ft.download(target)
        self.assertIn("download rejected", str(raised.exception))
        self.assertEqual(list(target.parent.iterdir()), [])


class PinnedReleaseTests(unittest.TestCase):
    @unittest.skipUnless(ft.CACHE.is_file(), "no cached Terrain3D zip, run tools/fetch_terrain3d.py")
    def test_the_cached_release_matches_the_pin(self):
        self.assertIsNone(ft.zip_problem(ft.CACHE))


if __name__ == "__main__":
    unittest.main()
