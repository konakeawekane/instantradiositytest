import math
from collections import defaultdict
from typing import Any, DefaultDict, Dict, List, Sequence, Tuple

from .color import (
    color_components,
    color_distance,
    color_matches,
    normalized_native_channels,
    normalized_rgba,
    normalized_to_hex,
    parse_hex_color,
    rgba_environment,
    validate_tolerance,
)
from .errors import RenderDebugError
from .images import LoadedImage
from .model import Query
from .predicate import evaluate_predicate, parse_predicate
from .selection import ResolvedSelection, pixel_uv_center, resolve_selection


EVALUATOR_IMAGE_COUNTS = {
    "pixel_value": 1,
    "average_color": 1,
    "min_max": 1,
    "percentiles": 1,
    "statistics": 1,
    "dominant_colors": 1,
    "color_distribution": 1,
    "nearest_color": 1,
    "color_count": 1,
    "color_range": 1,
    "histogram": 1,
    "occupancy": 1,
    "count": 1,
    "error": 2,
    "error_distribution": 2,
    "channel_error": 2,
    "color_difference": 2,
    "match": 2,
}


def _require_nonempty(selected: ResolvedSelection, evaluation: str) -> None:
    if selected.pixel_count == 0:
        raise RenderDebugError(
            "empty_selection", "%s requires at least one selected pixel" % evaluation
        )


def _channel_dict(names: Sequence[str], values: Sequence[float]) -> Dict[str, float]:
    return {name: value for name, value in zip(names, values)}


def _pixels(image: LoadedImage, selected: ResolvedSelection) -> List[Tuple[float, ...]]:
    return [image.pixel(x, y) for x, y in selected.coordinates()]


def _normalized_channel_columns(
    image: LoadedImage, selected: ResolvedSelection
) -> List[List[float]]:
    columns = [[] for _ in image.channel_names]
    for coordinate in selected.coordinates():
        normalized = normalized_native_channels(image, image.pixel(*coordinate))
        for index, value in enumerate(normalized):
            columns[index].append(value)
    return columns


def _percentile(values: Sequence[float], percentile: float) -> float:
    """Hyndman-Fan type 7: linear interpolation at (n - 1) * p / 100."""
    if not values:
        raise RenderDebugError("empty_selection", "Percentiles require at least one value")
    ordered = sorted(values)
    rank = (len(ordered) - 1) * percentile / 100.0
    lower = int(math.floor(rank))
    upper = int(math.ceil(rank))
    if lower == upper:
        return ordered[lower]
    fraction = rank - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def _validate_positive_integer(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise RenderDebugError(
            "invalid_parameter", "%s must be a positive integer" % name,
            {"parameter": name, "value": value},
        )
    return value


def _pixel_value(image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    if selected.pixel_count != 1:
        raise RenderDebugError(
            "invalid_selection_size", "pixel-value requires exactly one selected pixel",
            {"pixel_count": selected.pixel_count},
        )
    x, y = next(selected.coordinates())
    native = image.pixel(x, y)
    normalized = normalized_native_channels(image, native)
    result: Dict[str, Any] = {
        "pixel": {"x": x, "y": y},
        "uv_center": list(pixel_uv_center(x, y, image.width, image.height)),
        "uv_cell": {
            "min_inclusive": [x / image.width, y / image.height],
            "max_exclusive": [(x + 1) / image.width, (y + 1) / image.height],
        },
        "native_values": list(native),
        "channels": _channel_dict(image.channel_names, normalized),
        "channel_units": "normalized [0, 1] for bounded integer channels; native for unbounded/float channels",
    }
    if image.working_mode in {"L", "LA", "RGB", "RGBA"}:
        result["hex"] = normalized_to_hex(color_components(image, native))
    return result


def _average_color(image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    _require_nonempty(selected, "average-color")
    columns = _normalized_channel_columns(image, selected)
    means = [math.fsum(column) / selected.pixel_count for column in columns]
    return {"pixel_count": selected.pixel_count, "channels": _channel_dict(image.channel_names, means)}


def _min_max(image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    _require_nonempty(selected, "min-max")
    columns = _normalized_channel_columns(image, selected)
    return {
        "channels": {
            name: {"min": min(values), "max": max(values)}
            for name, values in zip(image.channel_names, columns)
        }
    }


def _percentiles(image: LoadedImage, selected: ResolvedSelection, requested: Sequence[float]) -> Dict[str, Any]:
    _require_nonempty(selected, "percentiles")
    columns = _normalized_channel_columns(image, selected)
    return {
        "method": "Hyndman-Fan type 7 linear interpolation: rank=(n-1)*p/100",
        "channels": {
            name: [{"percentile": value, "value": _percentile(column, value)} for value in requested]
            for name, column in zip(image.channel_names, columns)
        },
    }


def _statistics(image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    _require_nonempty(selected, "statistics")
    columns = _normalized_channel_columns(image, selected)
    result: Dict[str, Any] = {}
    for name, values in zip(image.channel_names, columns):
        mean = math.fsum(values) / len(values)
        variance = math.fsum((value - mean) ** 2 for value in values) / len(values)
        result[name] = {
            "count": len(values),
            "mean": mean,
            "median": _percentile(values, 50.0),
            "variance": variance,
            "standard_deviation": math.sqrt(variance),
            "minimum": min(values),
            "maximum": max(values),
        }
    return {"variance_kind": "population", "channels": result}


def _dominant_colors(image: LoadedImage, selected: ResolvedSelection, top: int) -> Dict[str, Any]:
    _require_nonempty(selected, "dominant-colors")
    clusters: DefaultDict[Tuple[int, ...], List[Any]] = defaultdict(lambda: [0, []])
    for x, y in selected.coordinates():
        components = color_components(image, image.pixel(x, y))
        key = tuple(max(0, min(31, int(math.floor(value * 32.0)))) for value in components)
        bucket = clusters[key]
        bucket[0] += 1
        if not bucket[1]:
            bucket[1] = [0.0 for _ in components]
        for index, value in enumerate(components):
            bucket[1][index] += value
    ordered = sorted(clusters.items(), key=lambda item: (-item[1][0], item[0]))[:top]
    colors = []
    for key, (count, totals) in ordered:
        representative = [value / count for value in totals]
        colors.append({
            "color": normalized_to_hex(representative),
            "normalized_channels": representative,
            "pixel_count": count,
            "percentage": count * 100.0 / selected.pixel_count,
            "cluster_key_5bit": list(key),
        })
    return {
        "clustering": "fixed 5-bit bins per RGB/RGBA channel: min(31, floor(channel*32))",
        "colors": colors,
    }


def _color_distribution(image: LoadedImage, selected: ResolvedSelection, samples: int) -> Dict[str, Any]:
    _require_nonempty(selected, "color-distribution")
    columns = _normalized_channel_columns(image, selected)
    points = [50.0] if samples == 1 else [index * 100.0 / (samples - 1) for index in range(samples)]
    return {
        "method": "even percentiles from 0 through 100 using type 7 interpolation; one sample uses p50",
        "channels": {
            name: [{"percentile": point, "value": _percentile(column, point)} for point in points]
            for name, column in zip(image.channel_names, columns)
        },
    }


def _nearest_color(image: LoadedImage, selected: ResolvedSelection, target_text: str) -> Dict[str, Any]:
    _require_nonempty(selected, "nearest-color")
    target = parse_hex_color(target_text)
    best = None
    for x, y in selected.coordinates():
        native = image.pixel(x, y)
        distance = color_distance(normalized_rgba(image, native), target)
        candidate = (distance, y, x, native)
        if best is None or candidate[:3] < best[:3]:
            best = candidate
    assert best is not None
    distance, y, x, native = best
    components = color_components(image, native)
    return {
        "target": normalized_to_hex(target),
        "pixel": {"x": x, "y": y},
        "uv_center": list(pixel_uv_center(x, y, image.width, image.height)),
        "color": normalized_to_hex(components),
        "normalized_channels": list(components),
        "distance": distance,
        "distance_metric": "root-mean-square normalized channel distance; RGB target ignores alpha",
    }


def _color_count(
    image: LoadedImage, selected: ResolvedSelection, target_text: str, tolerance: float
) -> Dict[str, Any]:
    target = parse_hex_color(target_text)
    count = sum(
        1 for x, y in selected.coordinates()
        if color_matches(image, image.pixel(x, y), target, tolerance)
    )
    percentage = count * 100.0 / selected.pixel_count if selected.pixel_count else 0.0
    return {
        "color": normalized_to_hex(target),
        "tolerance": tolerance,
        "distance_metric": "root-mean-square normalized channel distance; RGB target ignores alpha",
        "count": count,
        "total": selected.pixel_count,
        "percentage": percentage,
    }


def _color_range(image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    _require_nonempty(selected, "color-range")
    columns = _normalized_channel_columns(image, selected)
    return {
        "channels": {
            name: {"minimum": min(values), "maximum": max(values), "range": max(values) - min(values)}
            for name, values in zip(image.channel_names, columns)
        }
    }


def _histogram_values(values: Sequence[float], bins: int) -> Dict[str, Any]:
    counts = [0 for _ in range(bins)]
    underflow = 0
    overflow = 0
    for value in values:
        if value < 0.0:
            underflow += 1
        elif value > 1.0:
            overflow += 1
        else:
            index = min(int(math.floor(value * bins)), bins - 1)
            counts[index] += 1
    total = len(values)
    entries = [
        {
            "lower_inclusive": index / bins,
            "upper_exclusive": (index + 1) / bins if index < bins - 1 else None,
            "includes_upper_bound": index == bins - 1,
            "count": count,
            "percentage": count * 100.0 / total if total else 0.0,
        }
        for index, count in enumerate(counts)
    ]
    return {"bins": entries, "underflow": underflow, "overflow": overflow}


def _histogram(image: LoadedImage, selected: ResolvedSelection, bins: int) -> Dict[str, Any]:
    columns = _normalized_channel_columns(image, selected)
    return {
        "range": [0.0, 1.0],
        "binning": "[lower, upper), except final bin includes 1.0; out-of-range values are counted separately",
        "channels": {
            name: _histogram_values(values, bins)
            for name, values in zip(image.channel_names, columns)
        },
    }


def _occupancy(image: LoadedImage, selected: ResolvedSelection, condition: str) -> Dict[str, Any]:
    node = parse_predicate(condition)
    count = sum(
        1 for x, y in selected.coordinates()
        if evaluate_predicate(node, rgba_environment(image, image.pixel(x, y)))
    )
    total = selected.pixel_count
    fraction = count / total if total else 0.0
    return {"condition": condition, "count": count, "total": total, "fraction": fraction, "percentage": fraction * 100.0}


def _validate_pair(reference: LoadedImage, actual: LoadedImage) -> None:
    if (reference.width, reference.height) != (actual.width, actual.height):
        raise RenderDebugError(
            "incompatible_dimensions", "Images must have identical dimensions",
            {"reference": [reference.width, reference.height], "actual": [actual.width, actual.height]},
        )
    if reference.working_mode != actual.working_mode or reference.channel_names != actual.channel_names:
        raise RenderDebugError(
            "incompatible_channels", "Images must have identical working modes and channels",
            {"reference": reference.working_mode, "actual": actual.working_mode},
        )


def _pair_errors(
    reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection
) -> Tuple[List[List[float]], List[float]]:
    per_channel = [[] for _ in reference.channel_names]
    per_pixel = []
    for x, y in selected.coordinates():
        left = normalized_native_channels(reference, reference.pixel(x, y))
        right = normalized_native_channels(actual, actual.pixel(x, y))
        differences = [abs(a - b) for a, b in zip(left, right)]
        for index, difference in enumerate(differences):
            per_channel[index].append(difference)
        per_pixel.append(math.fsum(differences) / len(differences))
    return per_channel, per_pixel


def _error(reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    per_channel, per_pixel = _pair_errors(reference, actual, selected)
    totals = [math.fsum(values) for values in per_channel]
    total_error = math.fsum(totals)
    sample_count = selected.pixel_count * len(reference.channel_names)
    return {
        "metric": "absolute error on normalized channel values",
        "pixel_count": selected.pixel_count,
        "channel_count": len(reference.channel_names),
        "channel_sample_count": sample_count,
        "total_error": total_error,
        "mean_error": total_error / sample_count if sample_count else 0.0,
        "maximum_error": max((max(values) for values in per_channel if values), default=0.0),
        "per_channel_total_error": _channel_dict(reference.channel_names, totals),
        "per_channel_mean_error": _channel_dict(
            reference.channel_names,
            [value / selected.pixel_count if selected.pixel_count else 0.0 for value in totals],
        ),
    }


def _error_distribution(
    reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection, bins: int
) -> Dict[str, Any]:
    _, per_pixel = _pair_errors(reference, actual, selected)
    result = _histogram_values(per_pixel, bins)
    result.update({
        "metric": "per-pixel mean absolute error across normalized channels",
        "range": [0.0, 1.0],
    })
    return result


def _channel_error(reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    per_channel, _ = _pair_errors(reference, actual, selected)
    return {
        "metric": "absolute error on normalized channel values",
        "channels": {
            name: {
                "total": math.fsum(values),
                "mean": math.fsum(values) / len(values) if values else 0.0,
                "maximum": max(values, default=0.0),
            }
            for name, values in zip(reference.channel_names, per_channel)
        },
    }


def _color_difference(reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    _require_nonempty(selected, "color-difference")
    left_columns = _normalized_channel_columns(reference, selected)
    right_columns = _normalized_channel_columns(actual, selected)
    mean_difference: Dict[str, float] = {}
    distribution_difference: Dict[str, float] = {}
    for name, left, right in zip(reference.channel_names, left_columns, right_columns):
        mean_difference[name] = math.fsum(right) / len(right) - math.fsum(left) / len(left)
        distribution_difference[name] = math.fsum(
            abs(a - b) for a, b in zip(sorted(left), sorted(right))
        ) / len(left)
    return {
        "per_channel_mean_difference_actual_minus_reference": mean_difference,
        "per_channel_distribution_difference": distribution_difference,
        "distribution_metric": "1D Wasserstein distance: mean absolute difference of sorted equal-size channel populations",
    }


def _match(reference: LoadedImage, actual: LoadedImage, selected: ResolvedSelection, tolerance: float) -> Dict[str, Any]:
    per_channel, _ = _pair_errors(reference, actual, selected)
    maximum = max((max(values) for values in per_channel if values), default=0.0)
    return {
        "match": maximum <= tolerance,
        "tolerance": tolerance,
        "max_error": maximum,
        "metric": "maximum absolute error over corresponding normalized channel samples",
    }


def _single_evaluation(query: Query, image: LoadedImage, selected: ResolvedSelection) -> Dict[str, Any]:
    kind = query.evaluation.type
    parameters = query.evaluation.parameters
    if kind == "pixel_value":
        return _pixel_value(image, selected)
    if kind == "average_color":
        return _average_color(image, selected)
    if kind == "min_max":
        return _min_max(image, selected)
    if kind == "percentiles":
        return _percentiles(image, selected, parameters["percentiles"])
    if kind == "statistics":
        return _statistics(image, selected)
    if kind == "dominant_colors":
        return _dominant_colors(image, selected, parameters["top"])
    if kind == "color_distribution":
        return _color_distribution(image, selected, parameters["samples"])
    if kind == "nearest_color":
        return _nearest_color(image, selected, parameters["color"])
    if kind == "color_count":
        return _color_count(image, selected, parameters["color"], parameters["tolerance"])
    if kind == "color_range":
        return _color_range(image, selected)
    if kind == "histogram":
        return _histogram(image, selected, parameters["bins"])
    if kind == "occupancy":
        return _occupancy(image, selected, parameters["condition"])
    return {"count": selected.pixel_count}


def evaluate(query: Query, images: Sequence[LoadedImage]) -> Tuple[ResolvedSelection, Dict[str, Any]]:
    if query.evaluation.type not in EVALUATOR_IMAGE_COUNTS:
        raise RenderDebugError("unknown_evaluation", "Unsupported evaluation type", {"type": query.evaluation.type})
    expected = EVALUATOR_IMAGE_COUNTS[query.evaluation.type]
    if len(images) != expected:
        raise RenderDebugError(
            "invalid_image_count", "%s requires exactly %d image(s)" % (query.evaluation.type, expected),
            {"expected": expected, "actual": len(images)},
        )

    if expected == 1:
        selected = resolve_selection(query.selection, images[0])
        return selected, _single_evaluation(query, images[0], selected)

    reference, actual = images
    _validate_pair(reference, actual)
    selected = resolve_selection(query.selection, reference)
    actual_selection = resolve_selection(query.selection, actual)
    if selected.coordinate_tuple() != actual_selection.coordinate_tuple():
        raise RenderDebugError(
            "non_corresponding_selections",
            "The selector produced different pixel coordinates in the two images",
            {"reference_count": selected.pixel_count, "actual_count": actual_selection.pixel_count},
        )
    kind = query.evaluation.type
    parameters = query.evaluation.parameters
    if kind == "error":
        result = _error(reference, actual, selected)
    elif kind == "error_distribution":
        result = _error_distribution(reference, actual, selected, parameters["bins"])
    elif kind == "channel_error":
        result = _channel_error(reference, actual, selected)
    elif kind == "color_difference":
        result = _color_difference(reference, actual, selected)
    else:
        result = _match(reference, actual, selected, parameters["tolerance"])
    return selected, result
