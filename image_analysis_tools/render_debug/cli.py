import argparse
import json
import math
import sys
from typing import Any, Dict, List, Optional, Sequence, Tuple

from . import __version__
from .analysis import EVALUATOR_IMAGE_COUNTS, evaluate
from .color import parse_hex_color, validate_tolerance
from .errors import RenderDebugError
from .images import load_images
from .model import EvaluationQuery, Query, SelectionQuery
from .predicate import parse_predicate


SELECTORS = (
    "all", "pixel", "uv", "uv-rect", "pixel-rect", "uv-radius", "color", "predicate"
)
EVALUATORS = (
    "pixel-value", "average-color", "min-max", "percentiles", "statistics",
    "dominant-colors", "color-distribution", "nearest-color", "color-count",
    "color-range", "histogram", "occupancy", "count", "error",
    "error-distribution", "channel-error", "color-difference", "match",
)


class JSONArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise RenderDebugError("invalid_arguments", message)


def _float(value: str, name: str) -> float:
    try:
        numeric = float(value)
    except ValueError:
        raise RenderDebugError("invalid_arguments", "%s must be a number" % name, {"value": value})
    if not math.isfinite(numeric):
        raise RenderDebugError("invalid_arguments", "%s must be finite" % name, {"value": value})
    return numeric


def _integer(value: str, name: str) -> int:
    try:
        return int(value)
    except ValueError:
        raise RenderDebugError("invalid_arguments", "%s must be an integer" % name, {"value": value})


def _positive_integer(value: Optional[int], name: str) -> int:
    if value is None or isinstance(value, bool) or value <= 0:
        raise RenderDebugError(
            "invalid_parameter", "%s is required and must be a positive integer" % name,
            {"parameter": name, "value": value},
        )
    return value


def _exact_count(kind: str, values: Sequence[str], expected: int) -> None:
    if len(values) != expected:
        raise RenderDebugError(
            "invalid_arguments", "%s selector requires %d value(s)" % (kind, expected),
            {"actual": len(values)},
        )


def _parse_selection(tokens: Sequence[str]) -> SelectionQuery:
    if not tokens:
        raise RenderDebugError("invalid_arguments", "--select requires a selector")
    kind = tokens[0]
    values = list(tokens[1:])
    if kind not in SELECTORS:
        raise RenderDebugError("unknown_selection", "Unsupported selector", {"type": kind})
    if kind == "all":
        _exact_count(kind, values, 0)
        return SelectionQuery("all")
    if kind == "pixel":
        _exact_count(kind, values, 2)
        return SelectionQuery("pixel", {"x": _integer(values[0], "x"), "y": _integer(values[1], "y")})
    if kind == "uv":
        _exact_count(kind, values, 2)
        return SelectionQuery("uv", {"u": _float(values[0], "u"), "v": _float(values[1], "v")})
    if kind == "uv-rect":
        _exact_count(kind, values, 4)
        return SelectionQuery("uv_rect", {
            "min": [_float(values[0], "u_min"), _float(values[1], "v_min")],
            "max": [_float(values[2], "u_max"), _float(values[3], "v_max")],
        })
    if kind == "pixel-rect":
        _exact_count(kind, values, 4)
        return SelectionQuery("pixel_rect", {
            "x_min": _integer(values[0], "x_min"), "y_min": _integer(values[1], "y_min"),
            "x_max": _integer(values[2], "x_max"), "y_max": _integer(values[3], "y_max"),
        })
    if kind == "uv-radius":
        _exact_count(kind, values, 3)
        return SelectionQuery("uv_radius", {
            "u": _float(values[0], "u"), "v": _float(values[1], "v"),
            "radius": _float(values[2], "radius"),
        })
    if kind == "color":
        if len(values) not in {1, 2}:
            raise RenderDebugError("invalid_arguments", "color selector requires color and optional tolerance")
        parse_hex_color(values[0])
        tolerance = validate_tolerance(values[1] if len(values) == 2 else 0.0)
        return SelectionQuery("color", {"color": values[0].upper(), "tolerance": tolerance})
    _exact_count(kind, values, 1)
    parse_predicate(values[0])
    return SelectionQuery("predicate", {"expression": values[0]})


def _parse_percentiles(value: Optional[str]) -> List[float]:
    if value is None or not value.strip():
        raise RenderDebugError("invalid_parameter", "--percentiles is required")
    result = [_float(item.strip(), "percentile") for item in value.split(",")]
    if any(item < 0.0 or item > 100.0 for item in result):
        raise RenderDebugError("invalid_parameter", "Percentiles must be within [0, 100]")
    return result


def _reject_unused(namespace: argparse.Namespace, allowed: Sequence[str]) -> None:
    option_values = {
        "percentiles": namespace.percentiles,
        "top": namespace.top,
        "samples": namespace.samples,
        "color": namespace.color,
        "tolerance": namespace.tolerance,
        "bins": namespace.bins,
        "condition": namespace.condition,
    }
    unused = [name for name, value in option_values.items() if value is not None and name not in allowed]
    if unused:
        raise RenderDebugError(
            "invalid_parameter", "Evaluator does not accept supplied parameter(s)",
            {"parameters": sorted(unused)},
        )


def _parse_evaluation(tokens: Sequence[str], namespace: argparse.Namespace) -> EvaluationQuery:
    if len(tokens) != 1:
        raise RenderDebugError("invalid_arguments", "--evaluate requires exactly one evaluator name")
    public_name = tokens[0]
    if public_name not in EVALUATORS:
        raise RenderDebugError("unknown_evaluation", "Unsupported evaluator", {"type": public_name})
    parameters: Dict[str, Any] = {}
    allowed: List[str] = []
    if public_name == "percentiles":
        allowed = ["percentiles"]
        parameters["percentiles"] = _parse_percentiles(namespace.percentiles)
    elif public_name == "dominant-colors":
        allowed = ["top"]
        parameters["top"] = _positive_integer(namespace.top, "--top")
    elif public_name == "color-distribution":
        allowed = ["samples"]
        parameters["samples"] = _positive_integer(namespace.samples, "--samples")
    elif public_name == "nearest-color":
        allowed = ["color"]
        if namespace.color is None:
            raise RenderDebugError("invalid_parameter", "--color is required")
        parse_hex_color(namespace.color)
        parameters["color"] = namespace.color.upper()
    elif public_name == "color-count":
        allowed = ["color", "tolerance"]
        if namespace.color is None or namespace.tolerance is None:
            raise RenderDebugError("invalid_parameter", "color-count requires --color and --tolerance")
        parse_hex_color(namespace.color)
        parameters.update({
            "color": namespace.color.upper(),
            "tolerance": validate_tolerance(namespace.tolerance),
        })
    elif public_name in {"histogram", "error-distribution"}:
        allowed = ["bins"]
        parameters["bins"] = _positive_integer(namespace.bins, "--bins")
    elif public_name == "occupancy":
        allowed = ["condition"]
        if namespace.condition is None:
            raise RenderDebugError("invalid_parameter", "--condition is required")
        parse_predicate(namespace.condition)
        parameters["condition"] = namespace.condition
    elif public_name == "match":
        allowed = ["tolerance"]
        if namespace.tolerance is None:
            raise RenderDebugError("invalid_parameter", "--tolerance is required")
        parameters["tolerance"] = validate_tolerance(namespace.tolerance)
    _reject_unused(namespace, allowed)
    return EvaluationQuery(public_name.replace("-", "_"), parameters)


def build_parser() -> argparse.ArgumentParser:
    parser = JSONArgumentParser(prog="render-debug", description="Deterministic image analysis with JSON output.")
    parser.add_argument("--select", nargs="+", required=True, metavar="VALUE")
    parser.add_argument("--evaluate", nargs="+", required=True, metavar="VALUE")
    parser.add_argument("--images", nargs="+", required=True, metavar="PATH")
    parser.add_argument("--percentiles")
    parser.add_argument("--top", type=int)
    parser.add_argument("--samples", type=int)
    parser.add_argument("--color")
    parser.add_argument("--tolerance", type=float)
    parser.add_argument("--bins", type=int)
    parser.add_argument("--condition")
    parser.add_argument("--compact", action="store_true", help="Emit single-line JSON")
    parser.add_argument("--version", action="version", version=__version__)
    return parser


def parse_query(argv: Optional[Sequence[str]] = None) -> Tuple[Query, bool]:
    namespace = build_parser().parse_args(argv)
    query = Query(
        selection=_parse_selection(namespace.select),
        evaluation=_parse_evaluation(namespace.evaluate, namespace),
        images=list(namespace.images),
    )
    expected = EVALUATOR_IMAGE_COUNTS[query.evaluation.type]
    if len(query.images) != expected:
        raise RenderDebugError(
            "invalid_image_count", "%s requires exactly %d image(s)" % (query.evaluation.type, expected),
            {"expected": expected, "actual": len(query.images)},
        )
    return query, namespace.compact


def _emit(payload: Dict[str, Any], compact: bool, stream: Any) -> None:
    if compact:
        json.dump(payload, stream, separators=(",", ":"), sort_keys=True, allow_nan=False)
    else:
        json.dump(payload, stream, indent=2, sort_keys=True, allow_nan=False)
    stream.write("\n")


def run(argv: Optional[Sequence[str]] = None) -> Dict[str, Any]:
    query, _ = parse_query(argv)
    loaded = load_images(query.images)
    try:
        selected, result = evaluate(query, loaded)
        return {
            "ok": True,
            "query": query.to_dict(),
            "image_info": [image.info_dict() for image in loaded],
            "resolved_selection": selected.to_dict(),
            "result": result,
            "warnings": [warning for image in loaded for warning in image.warnings],
        }
    finally:
        for image in loaded:
            image.image.close()


def main(argv: Optional[Sequence[str]] = None) -> int:
    effective = list(argv) if argv is not None else sys.argv[1:]
    compact = "--compact" in effective
    try:
        _emit(run(effective), compact, sys.stdout)
        return 0
    except RenderDebugError as exc:
        _emit({"ok": False, "error": exc.to_dict(), "warnings": []}, compact, sys.stderr)
        return 2
    except Exception as exc:
        _emit({
            "ok": False,
            "error": {
                "code": "internal_error", "message": "Unexpected render-debug failure",
                "details": {"type": type(exc).__name__, "reason": str(exc)},
            },
            "warnings": [],
        }, compact, sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
