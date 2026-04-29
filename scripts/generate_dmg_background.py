#!/usr/bin/env python3

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont


REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_1X = REPO_ROOT / "src-tauri" / "dmg-background.png"
OUT_2X = REPO_ROOT / "src-tauri" / "dmg-background@2x.png"

SIZE_1X = (660, 400)
RETINA_SCALE = 2
MASTER_SCALE = 4
SIZE_2X = (SIZE_1X[0] * RETINA_SCALE, SIZE_1X[1] * RETINA_SCALE)

FONT_PATH = "/System/Library/Fonts/HelveticaNeue.ttc"
FONT_REGULAR = 0
FONT_BOLD = 1
FONT_LIGHT = 7


def lerp(a: int, b: int, t: float) -> int:
    return round(a + (b - a) * t)


def vertical_gradient(size: tuple[int, int], top: tuple[int, int, int], bottom: tuple[int, int, int]) -> Image.Image:
    width, height = size
    image = Image.new("RGBA", size)
    pixels = image.load()
    for y in range(height):
        t = y / max(height - 1, 1)
        row = tuple(lerp(top[i], bottom[i], t) for i in range(3)) + (255,)
        for x in range(width):
            pixels[x, y] = row
    return image


def scaled_box(box: tuple[float, float, float, float], scale: int) -> tuple[int, int, int, int]:
    return tuple(round(v * scale) for v in box)


def rounded_panel(
    image: Image.Image,
    box: tuple[float, float, float, float],
    radius: float,
    fill: tuple[int, int, int, int],
    blur: float,
    scale: int,
) -> None:
    overlay = Image.new("RGBA", image.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    draw.rounded_rectangle(scaled_box(box, scale), radius=round(radius * scale), fill=fill)
    overlay = overlay.filter(ImageFilter.GaussianBlur(radius=blur * scale))
    image.alpha_composite(overlay)


def solid_panel(
    draw: ImageDraw.ImageDraw,
    box: tuple[float, float, float, float],
    radius: float,
    fill: tuple[int, int, int, int],
    outline: tuple[int, int, int, int] | None,
    outline_width: float,
    scale: int,
) -> None:
    draw.rounded_rectangle(
        scaled_box(box, scale),
        radius=round(radius * scale),
        fill=fill,
        outline=outline,
        width=max(1, round(outline_width * scale)) if outline else 0,
    )


def ellipse_glow(
    image: Image.Image,
    box: tuple[float, float, float, float],
    fill: tuple[int, int, int, int],
    blur: float,
    scale: int,
) -> None:
    overlay = Image.new("RGBA", image.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    draw.ellipse(scaled_box(box, scale), fill=fill)
    overlay = overlay.filter(ImageFilter.GaussianBlur(radius=blur * scale))
    image.alpha_composite(overlay)


def centered_text(
    draw: ImageDraw.ImageDraw,
    center_x: float,
    top_y: float,
    text: str,
    font: ImageFont.FreeTypeFont,
    fill: tuple[int, int, int, int],
) -> None:
    left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
    width = right - left
    draw.text((center_x - width / 2, top_y), text, font=font, fill=fill)


def centered_multiline_text(
    draw: ImageDraw.ImageDraw,
    center_x: float,
    top_y: float,
    lines: list[str],
    font: ImageFont.FreeTypeFont,
    fill: tuple[int, int, int, int],
    line_gap: float,
) -> None:
    current_y = top_y
    for line in lines:
        centered_text(draw, center_x, current_y, line, font, fill)
        left, top, right, bottom = draw.textbbox((0, 0), line, font=font)
        current_y += (bottom - top) + line_gap


def draw_arrow(draw: ImageDraw.ImageDraw, scale: int) -> None:
    y = round(168 * scale)
    start_x = round(232 * scale)
    end_x = round(438 * scale)
    shaft_width = round(3.0 * scale)
    head = round(10 * scale)
    draw.line((start_x, y, end_x, y), fill=(244, 247, 252, 232), width=shaft_width)
    draw.polygon(
        [
            (end_x, y),
            (end_x - head, y - head // 2),
            (end_x - head, y + head // 2),
        ],
        fill=(244, 247, 252, 232),
    )


def build_background(scale: int) -> Image.Image:
    size = (SIZE_1X[0] * scale, SIZE_1X[1] * scale)
    image = vertical_gradient(size, (31, 35, 49), (20, 24, 35))

    rounded_panel(
        image,
        (68, 98, 592, 258),
        radius=42,
        fill=(255, 255, 255, 18),
        blur=18,
        scale=scale,
    )
    ellipse_glow(
        image,
        (104, 122, 258, 218),
        fill=(255, 255, 255, 44),
        blur=18,
        scale=scale,
    )
    ellipse_glow(
        image,
        (402, 122, 556, 218),
        fill=(255, 255, 255, 44),
        blur=18,
        scale=scale,
    )
    ellipse_glow(
        image,
        (176, 252, 484, 370),
        fill=(126, 149, 193, 30),
        blur=28,
        scale=scale,
    )

    draw = ImageDraw.Draw(image)
    draw_arrow(draw, scale)

    rounded_panel(
        image,
        (106, 216, 254, 246),
        radius=16,
        fill=(255, 255, 255, 36),
        blur=5,
        scale=scale,
    )
    rounded_panel(
        image,
        (406, 216, 554, 246),
        radius=16,
        fill=(255, 255, 255, 36),
        blur=5,
        scale=scale,
    )

    solid_panel(
        draw,
        (108, 219, 252, 243),
        radius=14,
        fill=(244, 247, 252, 224),
        outline=(255, 255, 255, 80),
        outline_width=0.8,
        scale=scale,
    )
    solid_panel(
        draw,
        (408, 219, 552, 243),
        radius=14,
        fill=(244, 247, 252, 224),
        outline=(255, 255, 255, 80),
        outline_width=0.8,
        scale=scale,
    )

    solid_panel(
        draw,
        (178, 340, 482, 374),
        radius=16,
        fill=(20, 24, 35, 120),
        outline=(255, 255, 255, 34),
        outline_width=0.8,
        scale=scale,
    )

    title_font = ImageFont.truetype(FONT_PATH, 26 * scale, index=FONT_BOLD)
    subtitle_font = ImageFont.truetype(FONT_PATH, 15 * scale, index=FONT_REGULAR)
    footer_font = ImageFont.truetype(FONT_PATH, 12 * scale, index=FONT_REGULAR)

    centered_text(
        draw,
        center_x=330 * scale,
        top_y=262 * scale,
        text="Drag Maekon to Applications",
        font=title_font,
        fill=(246, 248, 252, 248),
    )
    centered_text(
        draw,
        center_x=330 * scale,
        top_y=304 * scale,
        text="Drop the app icon onto the Applications folder",
        font=subtitle_font,
        fill=(205, 214, 229, 236),
    )
    centered_multiline_text(
        draw,
        center_x=330 * scale,
        top_y=347 * scale,
        lines=[
            "Installed when Maekon appears in Applications.",
            "Open it from Applications, then eject this disk image.",
        ],
        font=footer_font,
        fill=(224, 230, 241, 224),
        line_gap=5 * scale,
    )

    return image


def main() -> None:
    image_master = build_background(MASTER_SCALE)
    image_2x = image_master.resize(SIZE_2X, Image.Resampling.LANCZOS).filter(
        ImageFilter.UnsharpMask(radius=1.2, percent=145, threshold=2)
    )
    image_1x = image_2x.resize(SIZE_1X, Image.Resampling.LANCZOS).filter(
        ImageFilter.UnsharpMask(radius=1.0, percent=150, threshold=2)
    )

    OUT_2X.parent.mkdir(parents=True, exist_ok=True)
    image_2x.save(OUT_2X)
    image_1x.save(OUT_1X)

    print(f"wrote {OUT_1X}")
    print(f"wrote {OUT_2X}")


if __name__ == "__main__":
    main()
