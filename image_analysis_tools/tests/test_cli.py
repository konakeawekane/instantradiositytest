import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

from render_debug.cli import EVALUATORS, SELECTORS, main, parse_query, run
from render_debug.errors import RenderDebugError

from .helpers import save_pixels


class CLITests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.image = save_pixels(
            self.root / "image.png", (2, 2),
            [(0, 0, 0), (255, 0, 0), (0, 255, 0), (255, 255, 255)],
        )
        self.actual = save_pixels(
            self.root / "actual.png", (2, 2),
            [(0, 0, 0), (255, 0, 0), (0, 128, 0), (255, 255, 255)],
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_registry_exposes_exact_v1_names(self) -> None:
        self.assertEqual(8, len(SELECTORS))
        self.assertEqual(18, len(EVALUATORS))
        self.assertEqual(
            {"all", "pixel", "uv", "uv-rect", "pixel-rect", "uv-radius", "color", "predicate"},
            set(SELECTORS),
        )

    def test_pixel_value_complete_invocation(self) -> None:
        payload = run([
            "--select", "pixel", "1", "0",
            "--evaluate", "pixel-value",
            "--images", str(self.image),
        ])
        self.assertEqual("#FF0000", payload["result"]["hex"])
        self.assertEqual("pixel-value", payload["query"]["evaluation"]["type"])

    def test_uv_rect_average_complete_invocation(self) -> None:
        payload = run([
            "--select", "uv-rect", "0", "0", "0.5", "1",
            "--evaluate", "average-color",
            "--images", str(self.image),
        ])
        self.assertEqual(2, payload["resolved_selection"]["pixel_count"])
        self.assertEqual(0.5, payload["result"]["channels"]["g"])

    def test_uv_rect_error_complete_invocation(self) -> None:
        payload = run([
            "--select", "uv-rect", "0", "0", "1", "1",
            "--evaluate", "error",
            "--images", str(self.image), str(self.actual),
        ])
        self.assertAlmostEqual(127.0 / 255.0, payload["result"]["total_error"])

    def test_evaluator_flags_enter_normalized_query(self) -> None:
        query, compact = parse_query([
            "--select", "all",
            "--evaluate", "percentiles",
            "--percentiles", "1,25,50,99",
            "--images", str(self.image),
            "--compact",
        ])
        self.assertEqual([1.0, 25.0, 50.0, 99.0], query.evaluation.parameters["percentiles"])
        self.assertTrue(compact)

    def test_selector_inline_parameters_normalize(self) -> None:
        query, _ = parse_query([
            "--select", "color", "#FF0000", "0.02",
            "--evaluate", "count",
            "--images", str(self.image),
        ])
        self.assertEqual("color", query.selection.type)
        self.assertEqual(0.02, query.selection.parameters["tolerance"])

    def test_missing_required_evaluator_parameter_fails(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "--bins"):
            parse_query([
                "--select", "all", "--evaluate", "histogram", "--images", str(self.image)
            ])

    def test_irrelevant_evaluator_parameter_fails(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "does not accept"):
            parse_query([
                "--select", "all", "--evaluate", "count", "--bins", "4", "--images", str(self.image)
            ])

    def test_invalid_evaluator_parameter_ranges_fail(self) -> None:
        invalid_queries = [
            ["--evaluate", "percentiles", "--percentiles", "-1,50"],
            ["--evaluate", "dominant-colors", "--top", "0"],
            ["--evaluate", "color-distribution", "--samples", "0"],
            ["--evaluate", "histogram", "--bins", "0"],
            ["--evaluate", "nearest-color", "--color", "red"],
            ["--evaluate", "match", "--tolerance", "2", "--images", str(self.image), str(self.actual)],
        ]
        for partial in invalid_queries:
            arguments = ["--select", "all"] + partial
            if "--images" not in arguments:
                arguments += ["--images", str(self.image)]
            with self.assertRaises(RenderDebugError, msg=" ".join(arguments)):
                parse_query(arguments)

    def test_invalid_image_count_fails_before_loading(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "exactly 2"):
            parse_query([
                "--select", "all", "--evaluate", "error", "--images", str(self.image)
            ])

    def test_json_success_contains_no_nan_and_is_deterministic(self) -> None:
        arguments = [
            "--select", "all", "--evaluate", "statistics", "--images", str(self.image)
        ]
        first = json.dumps(run(arguments), sort_keys=True, allow_nan=False)
        second = json.dumps(run(arguments), sort_keys=True, allow_nan=False)
        self.assertEqual(first, second)

    def test_main_writes_json_error_to_stderr_and_nonzero_exit(self) -> None:
        stdout = StringIO()
        stderr = StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            exit_code = main([
                "--select", "pixel", "2", "0",
                "--evaluate", "pixel-value",
                "--images", str(self.image),
            ])
        self.assertEqual(2, exit_code)
        self.assertEqual("", stdout.getvalue())
        payload = json.loads(stderr.getvalue())
        self.assertEqual("pixel_out_of_bounds", payload["error"]["code"])

    def test_main_writes_valid_compact_json_success(self) -> None:
        stdout = StringIO()
        stderr = StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            exit_code = main([
                "--select", "all", "--evaluate", "count",
                "--images", str(self.image), "--compact",
            ])
        self.assertEqual(0, exit_code)
        self.assertEqual("", stderr.getvalue())
        self.assertEqual(4, json.loads(stdout.getvalue())["result"]["count"])
        self.assertEqual(1, len(stdout.getvalue().splitlines()))


if __name__ == "__main__":
    unittest.main()
