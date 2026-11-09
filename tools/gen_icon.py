#!/usr/bin/env python3
"""Builds crates/td_editor/assets/logo.ico, the Windows exe and installer icon, from logo.png.

  python tools/gen_icon.py

Needs Pillow.
"""
import pathlib

from PIL import Image

ASSETS = pathlib.Path(__file__).resolve().parent.parent / "crates" / "td_editor" / "assets"
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]

logo = Image.open(ASSETS / "logo.png").convert("RGBA")
logo.save(ASSETS / "logo.ico", sizes=[(s, s) for s in SIZES])
