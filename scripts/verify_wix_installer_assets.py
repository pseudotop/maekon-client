#!/usr/bin/env python3
"""Validate WiX source assets that protect the interactive installer layout."""

from __future__ import annotations

import struct
import sys
from pathlib import Path
from xml.etree import ElementTree

CLIENT_ROOT = Path(__file__).resolve().parents[1]
WIX_ROOT = CLIENT_ROOT / "src-tauri" / "wix"
BANNER = WIX_ROOT / "banner.bmp"
MANIFEST = WIX_ROOT / "main.wxs"
LOCALES = {"en-US": "1252", "ko-KR": "949"}
LOCALIZED_IDS = {
    "PackageDescription",
    "DowngradeErrorMessage",
    "DiskPrompt",
    "ShortcutDescription",
    "MainProgramDescription",
    "ShortcutsFeatureTitle",
}


def read_bmp(path: Path) -> tuple[int, int, list[list[tuple[int, int, int]]]]:
    data = path.read_bytes()
    if data[:2] != b"BM":
        raise AssertionError(f"{path} is not a Windows bitmap")
    pixel_offset = struct.unpack_from("<I", data, 10)[0]
    dib_size = struct.unpack_from("<I", data, 14)[0]
    if dib_size < 40:
        raise AssertionError("banner.bmp must use a BITMAPINFOHEADER")
    width, signed_height = struct.unpack_from("<ii", data, 18)
    planes, bits_per_pixel = struct.unpack_from("<HH", data, 26)
    compression = struct.unpack_from("<I", data, 30)[0]
    if planes != 1 or bits_per_pixel != 24 or compression != 0:
        raise AssertionError("banner.bmp must be an uncompressed 24-bit bitmap")
    height = abs(signed_height)
    stride = ((width * 3 + 3) // 4) * 4
    rows: list[list[tuple[int, int, int]]] = []
    for display_y in range(height):
        stored_y = height - 1 - display_y if signed_height > 0 else display_y
        start = pixel_offset + stored_y * stride
        row = []
        for x in range(width):
            blue, green, red = data[start + x * 3 : start + x * 3 + 3]
            row.append((red, green, blue))
        rows.append(row)
    return width, height, rows


def assert_banner_safe(path: Path = BANNER) -> None:
    width, height, rows = read_bmp(path)
    if (width, height) != (493, 58):
        raise AssertionError(f"banner.bmp must be 493x58, got {width}x{height}")

    unsafe = [
        (x, y, rows[y][x])
        for y in range(54)
        for x in range(400)
        if rows[y][x] != (255, 255, 255)
    ]
    if unsafe:
        x, y, color = unsafe[0]
        raise AssertionError(
            f"banner title safe area contains artwork at ({x}, {y}): {color}"
        )

    brand_pixels = [
        rows[y][x]
        for y in range(54)
        for x in range(400, width)
        if rows[y][x] != (255, 255, 255)
    ]
    if len(brand_pixels) < 100:
        raise AssertionError("banner right-side brand mark is missing")

    purple_rule = [
        color
        for y in range(54, height)
        for color in rows[y]
        if color[2] > color[0] and color[2] > color[1]
    ]
    if len(purple_rule) < width * 2:
        raise AssertionError("banner bottom brand rule is missing or incomplete")


def assert_manifest_contract() -> None:
    source = MANIFEST.read_text(encoding="utf-8")
    required = (
        "Language='$(var.InstallerLanguage)'",
        "Codepage='$(var.InstallerCodepage)'",
        "Languages='$(var.InstallerLanguage)'",
        "SummaryCodepage='$(var.InstallerCodepage)'",
        "<UI Id='MaekonUI_FeatureTree'>",
        "FaceName='Segoe UI' Size='9'",
        "FaceName='Segoe UI' Size='10' Bold='yes'",
        "<UIRef Id='WixUI_Common'/>",
    )
    missing = [token for token in required if token not in source]
    if missing:
        raise AssertionError(f"main.wxs is missing installer UI contracts: {missing}")
    if "<UIRef Id='WixUI_FeatureTree'/>" in source:
        raise AssertionError("main.wxs must not restore the Tahoma-based WixUI_FeatureTree UIRef")


def assert_locales() -> None:
    namespace = "{http://schemas.microsoft.com/wix/2006/localization}"
    parsed_ids: dict[str, set[str]] = {}
    for culture, codepage in LOCALES.items():
        root = ElementTree.parse(WIX_ROOT / f"{culture}.wxl").getroot()
        if root.attrib.get("Culture") != culture:
            raise AssertionError(f"{culture}.wxl has the wrong Culture")
        if root.attrib.get("Codepage") != codepage:
            raise AssertionError(f"{culture}.wxl has the wrong Codepage")
        parsed_ids[culture] = {
            node.attrib["Id"] for node in root.findall(f"{namespace}String")
        }
        if parsed_ids[culture] != LOCALIZED_IDS:
            raise AssertionError(f"{culture}.wxl localized string IDs drifted")

    korean = (WIX_ROOT / "ko-KR.wxl").read_text(encoding="utf-8")
    if not any("가" <= character <= "힣" for character in korean):
        raise AssertionError("ko-KR.wxl does not contain Korean UI text")


def main() -> int:
    try:
        assert_banner_safe()
        assert_manifest_contract()
        assert_locales()
    except (AssertionError, ElementTree.ParseError, OSError, struct.error) as error:
        print(f"WiX installer asset verification failed: {error}", file=sys.stderr)
        return 1
    print("WiX installer assets verified: safe banner, Segoe UI, en-US and ko-KR")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
