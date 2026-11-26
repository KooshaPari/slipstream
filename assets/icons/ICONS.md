# Slipstream icon system

Five hand-authored SVG icons give each Slipstream Windows binary its own
identity in Explorer, the taskbar and the system tray.

## Shared frame

Every icon sits on the same rounded-square frame so the set reads as one
family at a glance:

| Property | Value |
| --- | --- |
| Canvas | `viewBox="0 0 256 256"`, width and height `256` |
| Frame | `<rect x="16" y="16" width="224" height="224" rx="52">` |
| Fill | Linear gradient, top-left `#0B6FB5` to bottom-right `#062747` |
| Gradient id | Unique per file (`trayGrad`, `hostGrad`, `windowsGrad`, `probeGrad`, `cliGrad`) because each SVG is embedded separately |

The gradient id must stay unique across the five files. They are compiled
independently into separate binaries, and a shared id makes it ambiguous
which source a given gradient came from.

## Palette

| Role | Hex | Use |
| --- | --- | --- |
| Accent | `#32C3F5` | Highlights, indicator dots, primary detail |
| Glyph primary | `#F2FBFF` | Main glyph strokes and fills |
| Glyph secondary | `#7FD6F7` | Optional depth, interior detail |
| Gradient start | `#0B6FB5` | Top-left of the frame |
| Gradient end | `#062747` | Bottom-right of the frame |

Glyph strokes use `stroke-width="16"` with round line caps and round line
joins. Stroked paths carry no fill.

## Glyphs

| File | Binary | Depicts |
| --- | --- | --- |
| `tray.svg` | `slipstream-tray.exe` | A swoosh sweeping from lower-left to upper-right, a streaming ribbon |
| `host.svg` | `slipstream-host.exe` | Two stacked server blades with indicator dots and interior rails |
| `windows.svg` | `slipstream-windows.exe` | A display monitor on a stand, with a play triangle on the screen |
| `probe.svg` | `slipstream-probe.exe` | A semicircular gauge, needle angled up-right, hub dot, base line |
| `cli.svg` | `slipstream.exe` | A terminal window with a `>` chevron prompt and an underscore cursor |

## Regenerating

Requires Python 3 and ImageMagick (`magick`). The generator resolves the
binary with `shutil.which("magick")` first, then falls back to
`C:\Program Files\imagemagick-7.1.0-q16-hdri\magick.exe`.

```
python assets/icons/generate_icons.py
```

That renders each SVG at density 768, resizes to every frame size, and packs
the frames into a multi-frame `.ico`.

## Frame sizes

Every committed `.ico` must contain exactly these seven frames:

```
16, 24, 32, 48, 64, 128, 256
```

The generator parses the ICO directory itself after writing each file and
fails the run if the frame set is not exactly that list. A 256-pixel frame is
stored in the directory as width byte `0`, which the parser reads as `256`.

## Determinism

The generator writes PNG frames with `-define png:exclude-chunk=date,time` so
ImageMagick does not embed a creation timestamp, then packs those PNGs into
the ICO. Two runs therefore produce byte-identical `.ico` files.

```
python assets/icons/generate_icons.py --check
```

`--check` regenerates into a temporary directory, compares the SHA-256 of
each result against the committed `.ico`, and exits non-zero on any
difference. This lets CI prove the committed binaries still match the
committed SVG sources.

## Committed ICO hashes

| File | SHA-256 |
| --- | --- |
| `tray.ico` | `b85b1f71686a32a4a8988f36e746cd736337b8b1530f3b23a94c5f732bae19e5` |
| `host.ico` | `96ea9d09bd61f62f3a816b56024d682af0e7afe74466860e87df690162943028` |
| `windows.ico` | `58f40d67e00c5c70b6abb5de14e85f1608f3cf19963d5c8c8ac091ad8155bba8` |
| `probe.ico` | `8edf80ef14c00c4e293d95861dfa5ace06a1b13b2846aa0187479d6ea73e7a41` |
| `cli.ico` | `6363184745191f0106f2431ae156e941b12c9e53a5a7acc9a6204889b7013b29` |

## Embedded in the binaries

Each crate has a `build.rs` that embeds its own `.ico` through `winresource`,
so every Windows release exe carries an `RT_GROUP_ICON`, its seven `RT_ICON`
frames, and a `VERSIONINFO` block (`FileDescription`, `ProductName`,
`FileVersion`, `ProductVersion`). The build script resolves the icon as
`CARGO_MANIFEST_DIR/../../assets/icons/<name>.ico`, so it works regardless of
the directory cargo is invoked from. Non-Windows targets skip the resource
compiler entirely.

| Binary | Crate | Icon | 256px frame SHA-256 |
| --- | --- | --- | --- |
| `slipstream-host.exe` | `crates/slipstream-host` | `host.ico` | `2d4c41dbe833d46fd6fac6a325235a8539dd1cacbd01383f6936f050f684dc5f` |
| `slipstream-tray.exe` | `crates/slipstream-tray` | `tray.ico` | `010dfc41844325d70693887c935e067b5b99655cc42fa3601decf040c783314e` |
| `slipstream.exe` | `clients/cli` | `cli.ico` | `e753ff02f652fec74b9e0b5e5bb1097298a1d96c4d3b07843cc00c36be2c9408` |
| `slipstream-probe.exe` | `clients/probe` | `probe.ico` | `3b89e402f0a5100ad843f60846c0c5f7fd2d8739c919976bd52c07c1f3de7339` |
| `slipstream-windows.exe` | `clients/windows` | `windows.ico` | `5f7ae76cfca84913857b582c7788146019c822cf52bfc6fb8c70bfd5cecc7a91` |

The hash in the last column is the SHA-256 of the 256-pixel frame payload, so
it is stable across builds and machines. It also equals the hash of the
`RT_ICON` resource with id 7 that a release build embeds, which is how the
wiring is checked: parse the PE resource directory, hash each `RT_ICON` blob,
and compare frame 7 against the source `.ico`. All five values above are
distinct, so the five binaries really do carry five different icons.

## Source rules

The SVG sources are plain SVG with no scripting, no `foreignObject`, no
external references, no `xlink:href`, no web fonts, no embedded rasters, no
`<image>` element and no animation. Only `svg`, `defs`, `linearGradient`,
`stop`, `rect`, `path`, `circle` and `line` are used. The only URL that
appears is the required `http://www.w3.org/2000/svg` namespace declaration.
Each file stays under 4 KB.
