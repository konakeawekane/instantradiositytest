import math
import re
from typing import Dict, Sequence, Tuple

from .errors import RenderDebugError
from .images import LoadedImage


HEX_COLOR = re.compile(r"^#?([0-9A-Fa-f]{6}|[0-9A-Fa-f]{8})$")


def parse_hex_color(value: str) -> Tuple[float, ...]:
    match = HEX_COLOR.fullmatch(value)
    if not match:
        raise RenderDebugError(
            "invalid_color",
            "Color must be hexadecimal RGB (#RRGGBB) or RGBA (#RRGGBBAA)",
            {"value": value},
        )
    digits = match.group(1)
    return tuple(int(digits[index:index + 2], 16) / 255.0 for index in range(0, len(digits), 2))


def validate_tolerance(value: object) -> float:
    try:
        tolerance = float(value)
    except (TypeError, ValueError):
        raise RenderDebugError("invalid_tolerance", "Tolerance must be a number", {"value": value})
    if not math.isfinite(tolerance) or tolerance < 0.0 or tolerance > 1.0:
        raise RenderDebugError(
            "invalid_tolerance", "Tolerance must be finite and within [0, 1]", {"value": value}
        )
    return tolerance


def normalized_native_channels(image: LoadedImage, pixel: Sequence[float]) -> Tuple[float, ...]:
    if image.channel_max is None:
        values = tuple(float(value) for value in pixel)
    else:
        values = tuple(float(value) / image.channel_max for value in pixel)
    if any(not math.isfinite(value) for value in values):
        raise RenderDebugError("non_finite_channel", "Image contains a non-finite channel value")
    return values


def normalized_rgba(image: LoadedImage, pixel: Sequence[float]) -> Tuple[float, float, float, float]:
    values = normalized_native_channels(image, pixel)
    mode = image.working_mode
    if mode == "RGB":
        return values[0], values[1], values[2], 1.0
    if mode == "RGBA":
        return values[0], values[1], values[2], values[3]
    if mode == "L" or mode.startswith("I;") or mode in {"I", "F"}:
        return values[0], values[0], values[0], 1.0
    if mode == "LA":
        return values[0], values[0], values[0], values[1]
    if mode == "CMYK":
        cyan, magenta, yellow, black = values
        return (
            (1.0 - cyan) * (1.0 - black),
            (1.0 - magenta) * (1.0 - black),
            (1.0 - yellow) * (1.0 - black),
            1.0,
        )
    raise RenderDebugError("unsupported_color_mode", "Cannot derive RGB channels", {"mode": mode})


def color_components(image: LoadedImage, pixel: Sequence[float]) -> Tuple[float, ...]:
    rgba = normalized_rgba(image, pixel)
    return rgba if "a" in image.channel_names else rgba[:3]


def color_distance(pixel_rgba: Sequence[float], target: Sequence[float]) -> float:
    """Root-mean-square channel distance, RGB or RGBA according to target length."""
    compared = pixel_rgba[:len(target)]
    return math.sqrt(sum((left - right) ** 2 for left, right in zip(compared, target)) / len(target))


def color_matches(image: LoadedImage, pixel: Sequence[float], target: Sequence[float], tolerance: float) -> bool:
    return color_distance(normalized_rgba(image, pixel), target) <= tolerance


def rgba_environment(image: LoadedImage, pixel: Sequence[float]) -> Dict[str, float]:
    red, green, blue, alpha = normalized_rgba(image, pixel)
    return {"R": red, "G": green, "B": blue, "A": alpha}


def normalized_to_hex(values: Sequence[float]) -> str:
    components = [max(0, min(255, int(math.floor(value * 255.0 + 0.5)))) for value in values]
    return "#" + "".join("%02X" % component for component in components)
