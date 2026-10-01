import math
from dataclasses import dataclass, replace
from typing import Any, Dict, Iterator, Optional, Sequence, Tuple, Union

from .color import color_matches, parse_hex_color, rgba_environment, validate_tolerance
from .errors import RenderDebugError
from .images import LoadedImage
from .model import SelectionQuery
from .predicate import evaluate_predicate, parse_predicate


Coordinate = Tuple[int, int]


@dataclass(frozen=True)
class ResolvedSelection:
    type: str
    pixel_count: int
    bounds: Optional[Tuple[int, int, int, int]] = None
    point: Optional[Coordinate] = None
    explicit_coordinates: Optional[Tuple[Coordinate, ...]] = None
    image_width: int = 0
    image_height: int = 0
    pixel_values: Tuple[Tuple[float, ...], ...] = ()

    def coordinates(self) -> Iterator[Coordinate]:
        if self.explicit_coordinates is not None:
            yield from self.explicit_coordinates
            return
        if self.bounds is None:
            return
        x_min, y_min, x_max, y_max = self.bounds
        for y in range(y_min, y_max):
            for x in range(x_min, x_max):
                yield x, y

    def coordinate_tuple(self) -> Tuple[Coordinate, ...]:
        return tuple(self.coordinates())

    def to_dict(self) -> Dict[str, Any]:
        result: Dict[str, Any] = {
            "type": self.type.replace("_", "-"),
            "pixel_count": self.pixel_count,
            "image_dimensions": {"width": self.image_width, "height": self.image_height},
        }
        if self.bounds is not None:
            x_min, y_min, x_max, y_max = self.bounds
            result["pixel_bounds"] = {
                "min_inclusive": [x_min, y_min],
                "max_exclusive": [x_max, y_max],
                "width": x_max - x_min,
                "height": y_max - y_min,
            }
        if self.point is not None:
            result["pixel"] = list(self.point)
        return result


def _validate_uv(value: Any, name: str) -> float:
    try:
        numeric = float(value)
    except (TypeError, ValueError):
        raise RenderDebugError("invalid_coordinate", "%s must be a number" % name, {"parameter": name})
    if not math.isfinite(numeric) or numeric < 0.0 or numeric > 1.0:
        raise RenderDebugError(
            "invalid_coordinate", "%s must be finite and within [0, 1]" % name,
            {"parameter": name, "value": value},
        )
    return numeric


def _validate_integer(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise RenderDebugError(
            "invalid_coordinate", "%s must be an integer" % name,
            {"parameter": name, "value": value},
        )
    return value


def uv_to_pixel(u: float, v: float, width: int, height: int) -> Coordinate:
    u = _validate_uv(u, "u")
    v = _validate_uv(v, "v")
    return min(int(math.floor(u * width)), width - 1), min(int(math.floor(v * height)), height - 1)


def pixel_uv_center(x: int, y: int, width: int, height: int) -> Tuple[float, float]:
    return (x + 0.5) / width, (y + 0.5) / height


def _dimensions(image_or_width: Union[LoadedImage, int], height: Optional[int]) -> Tuple[int, int, Optional[LoadedImage]]:
    if isinstance(image_or_width, LoadedImage):
        return image_or_width.width, image_or_width.height, image_or_width
    if height is None:
        raise RenderDebugError("invalid_image", "Image height is required")
    return image_or_width, height, None


def _explicit(kind: str, coordinates: Sequence[Coordinate]) -> ResolvedSelection:
    ordered = tuple(coordinates)
    return ResolvedSelection(kind, len(ordered), explicit_coordinates=ordered)


def _resolve_selection(
    selection: SelectionQuery,
    image_or_width: Union[LoadedImage, int],
    height: Optional[int] = None,
) -> ResolvedSelection:
    width, image_height, image = _dimensions(image_or_width, height)
    if width <= 0 or image_height <= 0:
        raise RenderDebugError("invalid_image", "Image dimensions must be positive")
    kind = selection.type
    parameters = selection.parameters

    if kind == "all":
        return ResolvedSelection(kind, width * image_height, (0, 0, width, image_height))

    if kind == "pixel":
        x = _validate_integer(parameters.get("x"), "x")
        y = _validate_integer(parameters.get("y"), "y")
        if x < 0 or x >= width or y < 0 or y >= image_height:
            raise RenderDebugError(
                "pixel_out_of_bounds", "Pixel coordinate is outside the image",
                {"pixel": [x, y], "dimensions": [width, image_height]},
            )
        return ResolvedSelection(kind, 1, (x, y, x + 1, y + 1), (x, y))

    if kind == "uv":
        x, y = uv_to_pixel(parameters.get("u"), parameters.get("v"), width, image_height)
        return ResolvedSelection(kind, 1, (x, y, x + 1, y + 1), (x, y))

    if kind == "pixel_rect":
        x_min = _validate_integer(parameters.get("x_min"), "x_min")
        y_min = _validate_integer(parameters.get("y_min"), "y_min")
        x_max = _validate_integer(parameters.get("x_max"), "x_max")
        y_max = _validate_integer(parameters.get("y_max"), "y_max")
        if any((x_min < 0, y_min < 0, x_max < 0, y_max < 0, x_min >= width, x_max >= width, y_min >= image_height, y_max >= image_height)):
            raise RenderDebugError(
                "pixel_rect_out_of_bounds", "pixel-rect coordinates must be inside the image",
                {"min": [x_min, y_min], "max": [x_max, y_max], "dimensions": [width, image_height]},
            )
        if x_min > x_max or y_min > y_max:
            raise RenderDebugError("invalid_pixel_rect", "pixel-rect minimums must not exceed maximums")
        return ResolvedSelection(
            kind, (x_max - x_min + 1) * (y_max - y_min + 1),
            (x_min, y_min, x_max + 1, y_max + 1),
        )

    if kind == "uv_rect":
        minimum = parameters.get("min")
        maximum = parameters.get("max")
        if not isinstance(minimum, (list, tuple)) or len(minimum) != 2 or not isinstance(maximum, (list, tuple)) or len(maximum) != 2:
            raise RenderDebugError("invalid_uv_rect", "uv-rect min and max must each contain two values")
        u_min, v_min = _validate_uv(minimum[0], "u_min"), _validate_uv(minimum[1], "v_min")
        u_max, v_max = _validate_uv(maximum[0], "u_max"), _validate_uv(maximum[1], "v_max")
        if u_min > u_max or v_min > v_max:
            raise RenderDebugError("invalid_uv_rect", "uv-rect minimums must not exceed maximums")
        if u_min == u_max or v_min == v_max:
            return _explicit(kind, ())
        xs = [x for x in range(width) if u_min <= (x + 0.5) / width <= u_max]
        ys = [y for y in range(image_height) if v_min <= (y + 0.5) / image_height <= v_max]
        if not xs or not ys:
            return _explicit(kind, ())
        return ResolvedSelection(kind, len(xs) * len(ys), (xs[0], ys[0], xs[-1] + 1, ys[-1] + 1))

    if kind == "uv_radius":
        u = _validate_uv(parameters.get("u"), "u")
        v = _validate_uv(parameters.get("v"), "v")
        try:
            radius = float(parameters.get("radius"))
        except (TypeError, ValueError):
            raise RenderDebugError("invalid_radius", "Radius must be a number")
        if not math.isfinite(radius) or radius < 0.0:
            raise RenderDebugError("invalid_radius", "Radius must be finite and non-negative")
        coordinates = []
        radius_squared = radius * radius
        for y in range(image_height):
            for x in range(width):
                center_u, center_v = pixel_uv_center(x, y, width, image_height)
                if (center_u - u) ** 2 + (center_v - v) ** 2 <= radius_squared:
                    coordinates.append((x, y))
        return _explicit(kind, coordinates)

    if kind in {"color", "predicate"}:
        if image is None:
            raise RenderDebugError("image_required", "%s selection requires pixel data" % kind)
        coordinates = []
        if kind == "color":
            target = parse_hex_color(parameters.get("color", ""))
            tolerance = validate_tolerance(parameters.get("tolerance", 0.0))
            for y in range(image_height):
                for x in range(width):
                    if color_matches(image, image.pixel(x, y), target, tolerance):
                        coordinates.append((x, y))
        else:
            node = parse_predicate(parameters.get("expression"))
            for y in range(image_height):
                for x in range(width):
                    if evaluate_predicate(node, rgba_environment(image, image.pixel(x, y))):
                        coordinates.append((x, y))
        return _explicit(kind, coordinates)

    raise RenderDebugError("unknown_selection", "Unsupported selection type", {"type": kind})


def resolve_selection(
    selection: SelectionQuery,
    image_or_width: Union[LoadedImage, int],
    height: Optional[int] = None,
) -> ResolvedSelection:
    result = _resolve_selection(selection, image_or_width, height)
    width, image_height, image = _dimensions(image_or_width, height)
    coordinates = result.coordinate_tuple()
    values = tuple(image.pixel(x, y) for x, y in coordinates) if image is not None else ()
    return replace(
        result,
        explicit_coordinates=coordinates,
        image_width=width,
        image_height=image_height,
        pixel_values=values,
    )
