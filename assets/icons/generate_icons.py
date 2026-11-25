#!/usr/bin/env python3
"""Render the Slipstream SVG icon sources into reproducible multi-frame Windows ICO files.

Usage:
    python assets/icons/generate_icons.py            # write assets/icons/*.ico
    python assets/icons/generate_icons.py --check    # verify committed .ico match

The generator uses only the Python standard library and shells out to
ImageMagick for rasterisation. Output is deterministic: two runs produce
byte-identical .ico files, so `--check` can prove that the committed binaries
still match the committed SVG sources.

Each .ico must contain exactly the frame sizes 16, 24, 32, 48, 64, 128 and 256.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ICON_DIR = Path(__file__).resolve().parent

ICON_NAMES = ("tray", "host", "windows", "probe", "cli")
FRAME_SIZES = (16, 24, 32, 48, 64, 128, 256)

FALLBACK_MAGICK = r"C:\Program Files\imagemagick-7.1.0-q16-hdri\magick.exe"

# High density so the 256 px frame is rasterised sharply before downscaling.
RASTER_DENSITY = "768"


def resolve_magick() -> str:
    """Locate the ImageMagick binary, preferring whatever is on PATH."""
    found = shutil.which("magick")
    if found:
        return found
    if Path(FALLBACK_MAGICK).is_file():
        return FALLBACK_MAGICK
    raise SystemExit(
        "ImageMagick not found. Install it or put 'magick' on PATH, then re-run."
    )


def run_magick(magick: str, args: list[str]) -> None:
    subprocess.run([magick, *args], check=True)


def render_frames(magick: str, svg: Path, workdir: Path, stem: str) -> list[Path]:
    """Rasterise one SVG at every frame size, returning the PNG paths."""
    frames: list[Path] = []
    for size in FRAME_SIZES:
        png = workdir / f"{stem}-{size}.png"
        run_magick(
            magick,
            [
                "-background",
                "none",
                "-density",
                RASTER_DENSITY,
                str(svg),
                "-resize",
                f"{size}x{size}",
                # Strip creation-date chunks so repeated runs are byte-identical.
                "-define",
                "png:exclude-chunk=date,time",
                str(png),
            ],
        )
        if not png.is_file():
            raise SystemExit(f"ImageMagick did not produce {png}")
        frames.append(png)
    return frames


def read_ico_frames(path: Path) -> list[tuple[int, int]]:
    """Parse an ICO directory and return the (width, height) of every frame."""
    data = path.read_bytes()
    if len(data) < 6:
        raise SystemExit(f"{path} is too small to be an ICO")
    reserved = int.from_bytes(data[0:2], "little")
    image_type = int.from_bytes(data[2:4], "little")
    count = int.from_bytes(data[4:6], "little")
    if reserved != 0 or image_type != 1:
        raise SystemExit(f"{path} is not a valid ICO (reserved={reserved} type={image_type})")
    if 6 + count * 16 > len(data):
        raise SystemExit(f"{path} directory is truncated (count={count})")
    frames: list[tuple[int, int]] = []
    for index in range(count):
        entry = data[6 + index * 16 : 6 + index * 16 + 16]
        width = entry[0] or 256
        height = entry[1] or 256
        frames.append((width, height))
    return frames


def assert_frame_sizes(path: Path) -> list[tuple[int, int]]:
    frames = read_ico_frames(path)
    expected = {(size, size) for size in FRAME_SIZES}
    if set(frames) != expected or len(frames) != len(FRAME_SIZES):
        raise SystemExit(
            f"{path.name} has frames {frames}, expected exactly "
            f"{sorted(expected)}"
        )
    return frames


def build_ico(magick: str, name: str, outdir: Path, workdir: Path) -> Path:
    svg = ICON_DIR / f"{name}.svg"
    if not svg.is_file():
        raise SystemExit(f"missing source {svg}")
    frames = render_frames(magick, svg, workdir, name)
    ico = outdir / f"{name}.ico"
    run_magick(magick, [str(frame) for frame in frames] + [str(ico)])
    if not ico.is_file():
        raise SystemExit(f"ImageMagick did not produce {ico}")
    assert_frame_sizes(ico)
    return ico


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def generate(magick: str, outdir: Path) -> dict[str, Path]:
    """Render every icon into outdir, returning the written .ico paths."""
    produced: dict[str, Path] = {}
    with tempfile.TemporaryDirectory() as frames_dir:
        workdir = Path(frames_dir)
        for name in ICON_NAMES:
            produced[name] = build_ico(magick, name, outdir, workdir)
    return produced


def check(magick: str) -> int:
    """Regenerate into a temp dir and compare hashes with the committed files."""
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        regenerated = generate(magick, Path(tmp))
        for name in ICON_NAMES:
            committed = ICON_DIR / f"{name}.ico"
            if not committed.is_file():
                print(f"FAIL {name}: committed {committed.name} is missing")
                failures += 1
                continue
            expected = sha256(regenerated[name])
            actual = sha256(committed)
            status = "ok" if expected == actual else "FAIL"
            if expected != actual:
                failures += 1
            print(f"{status} {name}.ico sha256={actual}")
            if expected != actual:
                print(f"     regenerated sha256={expected}")
    if failures:
        print(f"check failed: {failures} of {len(ICON_NAMES)} icons differ")
        return 1
    print(f"check passed: all {len(ICON_NAMES)} icons match their SVG sources")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate into a temporary directory and compare against the committed .ico files",
    )
    args = parser.parse_args(argv)

    magick = resolve_magick()

    if args.check:
        return check(magick)

    produced = generate(magick, ICON_DIR)

    print(f"wrote {len(produced)} icons to {ICON_DIR}")
    for name in ICON_NAMES:
        path = produced[name]
        frames = assert_frame_sizes(path)
        sizes = ", ".join(str(width) for width, _ in frames)
        print(f"  {path.name}: frames=[{sizes}] sha256={sha256(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
