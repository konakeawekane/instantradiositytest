import math
import tempfile
import unittest
from pathlib import Path

from PIL import Image

from render_debug.analysis import EVALUATOR_IMAGE_COUNTS, evaluate
from render_debug.errors import RenderDebugError
from render_debug.images import load_images
from render_debug.model import EvaluationQuery, Query, SelectionQuery

from .helpers import save_pixels


class AnalysisTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.rgb = save_pixels(
            self.root / "known.png", (2, 2),
            [(0, 0, 0), (255, 0, 0), (0, 255, 0), (255, 255, 255)],
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def evaluate_paths(self, query: Query):
        images = load_images(query.images)
        try:
            return evaluate(query, images)
        finally:
            for image in images:
                image.image.close()

    def single(self, evaluation: str, parameters=None, selection=None, image=None):
        query = Query(
            selection or SelectionQuery("all"),
            EvaluationQuery(evaluation, parameters or {}),
            [str(image or self.rgb)],
        )
        return self.evaluate_paths(query)

    def pair(self, evaluation: str, reference: Path, actual: Path, parameters=None, selection=None):
        query = Query(
            selection or SelectionQuery("all"),
            EvaluationQuery(evaluation, parameters or {}),
            [str(reference), str(actual)],
        )
        return self.evaluate_paths(query)

    def test_v1_registry_has_exactly_eighteen_evaluators(self) -> None:
        self.assertEqual(18, len(EVALUATOR_IMAGE_COUNTS))
        self.assertEqual(5, sum(count == 2 for count in EVALUATOR_IMAGE_COUNTS.values()))

    def test_pixel_value_returns_exact_native_hex_and_normalized_channels(self) -> None:
        path = save_pixels(self.root / "pixel.png", (1, 1), [(127, 163, 66)])
        _, result = self.single(
            "pixel_value", selection=SelectionQuery("pixel", {"x": 0, "y": 0}), image=path
        )
        self.assertEqual([127, 163, 66], result["native_values"])
        self.assertEqual("#7FA342", result["hex"])
        self.assertAlmostEqual(127 / 255.0, result["channels"]["r"])

    def test_pixel_value_accepts_any_single_pixel_selection_and_rejects_other_sizes(self) -> None:
        _, result = self.single(
            "pixel_value",
            selection=SelectionQuery("pixel_rect", {"x_min": 1, "y_min": 1, "x_max": 1, "y_max": 1}),
        )
        self.assertEqual({"x": 1, "y": 1}, result["pixel"])
        with self.assertRaisesRegex(RenderDebugError, "exactly one"):
            self.single("pixel_value")

    def test_float_pixel_preserves_native_value_without_hex(self) -> None:
        path = self.root / "float.tiff"
        image = Image.new("F", (1, 1), 0.625)
        image.save(str(path))
        image.close()
        _, result = self.single(
            "pixel_value", selection=SelectionQuery("pixel", {"x": 0, "y": 0}), image=path
        )
        self.assertEqual(0.625, result["channels"]["value"])
        self.assertNotIn("hex", result)

    def test_average_color_is_normalized_channel_arithmetic_mean_with_alpha(self) -> None:
        path = save_pixels(
            self.root / "alpha.png", (2, 1), [(0, 0, 0, 0), (255, 255, 255, 255)], "RGBA"
        )
        _, result = self.single("average_color", image=path)
        self.assertEqual({"r": 0.5, "g": 0.5, "b": 0.5, "a": 0.5}, result["channels"])

    def test_min_max_returns_each_available_channel(self) -> None:
        _, result = self.single("min_max")
        self.assertEqual({"min": 0.0, "max": 1.0}, result["channels"]["r"])
        self.assertEqual({"min": 0.0, "max": 1.0}, result["channels"]["b"])

    def test_percentiles_use_type_seven_linear_interpolation(self) -> None:
        path = save_pixels(self.root / "gray.png", (4, 1), [0, 85, 170, 255], "L")
        _, result = self.single("percentiles", {"percentiles": [0.0, 25.0, 50.0, 75.0, 100.0]}, image=path)
        values = [entry["value"] for entry in result["channels"]["gray"]]
        self.assertEqual([0.0, 0.25, 0.5, 0.75, 1.0], values)

    def test_statistics_use_population_variance(self) -> None:
        path = save_pixels(self.root / "gray.png", (4, 1), [0, 85, 170, 255], "L")
        _, result = self.single("statistics", image=path)
        gray = result["channels"]["gray"]
        self.assertEqual(4, gray["count"])
        self.assertEqual(0.5, gray["mean"])
        self.assertEqual(0.5, gray["median"])
        self.assertAlmostEqual(5.0 / 36.0, gray["variance"])
        self.assertAlmostEqual(math.sqrt(5.0) / 6.0, gray["standard_deviation"])

    def test_dominant_colors_group_fixed_five_bit_bins_and_order_ties(self) -> None:
        path = save_pixels(
            self.root / "dominant.png", (4, 1),
            [(0, 0, 0), (0, 0, 0), (255, 0, 0), (254, 0, 0)],
        )
        _, result = self.single("dominant_colors", {"top": 2}, image=path)
        colors = result["colors"]
        self.assertEqual(["#000000", "#FF0000"], [item["color"] for item in colors])
        self.assertEqual([2, 2], [item["pixel_count"] for item in colors])
        self.assertEqual([50.0, 50.0], [item["percentage"] for item in colors])

    def test_color_distribution_returns_exact_sample_count(self) -> None:
        path = save_pixels(self.root / "gray.png", (4, 1), [0, 85, 170, 255], "L")
        _, result = self.single("color_distribution", {"samples": 3}, image=path)
        samples = result["channels"]["gray"]
        self.assertEqual([0.0, 50.0, 100.0], [sample["percentile"] for sample in samples])
        self.assertEqual([0.0, 0.5, 1.0], [sample["value"] for sample in samples])
        _, one = self.single("color_distribution", {"samples": 1}, image=path)
        self.assertEqual(50.0, one["channels"]["gray"][0]["percentile"])

    def test_nearest_color_uses_row_major_tie_break(self) -> None:
        path = save_pixels(self.root / "tie.png", (2, 1), [(254, 0, 0), (254, 0, 0)])
        _, result = self.single("nearest_color", {"color": "#FF0000"}, image=path)
        self.assertEqual({"x": 0, "y": 0}, result["pixel"])
        self.assertAlmostEqual(1.0 / (255.0 * math.sqrt(3)), result["distance"])

    def test_color_count_uses_shared_distance_and_empty_percentage(self) -> None:
        path = save_pixels(self.root / "colors.png", (2, 1), [(255, 0, 0), (254, 0, 0)])
        _, exact = self.single("color_count", {"color": "#FF0000", "tolerance": 0.0}, image=path)
        self.assertEqual(1, exact["count"])
        _, near = self.single("color_count", {"color": "#FF0000", "tolerance": 0.003}, image=path)
        self.assertEqual(2, near["count"])
        empty = SelectionQuery("uv_radius", {"u": 0, "v": 0, "radius": 0})
        _, result = self.single(
            "color_count", {"color": "#FF0000", "tolerance": 0.0}, selection=empty, image=path
        )
        self.assertEqual(0.0, result["percentage"])

    def test_color_range_returns_minimum_maximum_and_range(self) -> None:
        _, result = self.single("color_range")
        self.assertEqual({"minimum": 0.0, "maximum": 1.0, "range": 1.0}, result["channels"]["g"])

    def test_histogram_uses_fixed_half_open_bins_with_final_upper_inclusive(self) -> None:
        path = save_pixels(self.root / "gray.png", (4, 1), [0, 85, 170, 255], "L")
        _, result = self.single("histogram", {"bins": 2}, image=path)
        bins = result["channels"]["gray"]["bins"]
        self.assertEqual([2, 2], [item["count"] for item in bins])
        self.assertTrue(bins[-1]["includes_upper_bound"])

    def test_occupancy_evaluates_condition_on_existing_selection(self) -> None:
        _, result = self.single("occupancy", {"condition": "R > 0.5 AND B < 0.5"})
        self.assertEqual(1, result["count"])
        self.assertEqual(4, result["total"])
        self.assertEqual(0.25, result["fraction"])

    def test_count_returns_selection_size_including_zero(self) -> None:
        _, result = self.single("count", selection=SelectionQuery("uv_radius", {"u": 0, "v": 0, "radius": 0}))
        self.assertEqual(0, result["count"])

    def comparison_pair(self):
        reference = save_pixels(
            self.root / "reference.png", (2, 1), [(0, 0, 0), (0, 0, 0)]
        )
        actual = save_pixels(
            self.root / "actual.png", (2, 1), [(0, 0, 0), (255, 128, 0)]
        )
        return reference, actual

    def test_error_returns_normalized_total_channel_and_maximum_metrics(self) -> None:
        reference, actual = self.comparison_pair()
        _, result = self.pair("error", reference, actual)
        expected_total = 1.0 + 128.0 / 255.0
        self.assertEqual(2, result["pixel_count"])
        self.assertEqual(3, result["channel_count"])
        self.assertAlmostEqual(expected_total, result["total_error"])
        self.assertEqual(0.0, result["per_channel_total_error"]["b"])
        self.assertAlmostEqual(128.0 / 255.0, result["per_channel_total_error"]["g"])
        self.assertEqual(1.0, result["maximum_error"])

    def test_error_distribution_uses_same_per_pixel_absolute_error(self) -> None:
        reference = save_pixels(self.root / "black.png", (2, 1), [(0, 0, 0), (0, 0, 0)])
        actual = save_pixels(self.root / "mixed.png", (2, 1), [(0, 0, 0), (255, 255, 255)])
        _, result = self.pair("error_distribution", reference, actual, {"bins": 2})
        self.assertEqual([1, 1], [item["count"] for item in result["bins"]])

    def test_channel_error_includes_alpha_when_present(self) -> None:
        reference = save_pixels(self.root / "ra.png", (1, 1), [(0, 0, 0, 0)], "RGBA")
        actual = save_pixels(self.root / "aa.png", (1, 1), [(0, 0, 0, 255)], "RGBA")
        _, result = self.pair("channel_error", reference, actual)
        self.assertEqual({"total": 1.0, "mean": 1.0, "maximum": 1.0}, result["channels"]["a"])
        self.assertEqual(0.0, result["channels"]["r"]["total"])

    def test_color_difference_reports_signed_mean_and_wasserstein_distribution(self) -> None:
        reference = save_pixels(self.root / "ref-gray.png", (2, 1), [0, 255], "L")
        actual = save_pixels(self.root / "act-gray.png", (2, 1), [64, 191], "L")
        _, result = self.pair("color_difference", reference, actual)
        self.assertEqual(0.0, result["per_channel_mean_difference_actual_minus_reference"]["gray"])
        self.assertAlmostEqual(64.0 / 255.0, result["per_channel_distribution_difference"]["gray"])

    def test_match_uses_maximum_corresponding_channel_error(self) -> None:
        reference = save_pixels(self.root / "ref.png", (1, 1), [(10, 10, 10)])
        actual = save_pixels(self.root / "act.png", (1, 1), [(11, 10, 10)])
        _, passing = self.pair("match", reference, actual, {"tolerance": 0.004})
        _, failing = self.pair("match", reference, actual, {"tolerance": 0.003})
        self.assertTrue(passing["match"])
        self.assertFalse(failing["match"])

    def test_empty_selection_behavior_is_explicit(self) -> None:
        empty = SelectionQuery("uv_radius", {"u": 0, "v": 0, "radius": 0})
        with self.assertRaisesRegex(RenderDebugError, "requires at least one"):
            self.single("average_color", selection=empty)
        _, histogram = self.single("histogram", {"bins": 2}, selection=empty)
        self.assertEqual([0, 0], [item["count"] for item in histogram["channels"]["r"]["bins"]])
        _, match = self.pair("match", self.rgb, self.rgb, {"tolerance": 0}, selection=empty)
        self.assertTrue(match["match"])

    def test_pair_rejects_different_dimensions_modes_and_content_selections(self) -> None:
        different_size = save_pixels(self.root / "size.png", (1, 1), [(0, 0, 0)])
        with self.assertRaisesRegex(RenderDebugError, "identical dimensions"):
            self.pair("error", self.rgb, different_size)
        gray = save_pixels(self.root / "gray.png", (2, 2), [0, 0, 0, 0], "L")
        with self.assertRaisesRegex(RenderDebugError, "identical working modes"):
            self.pair("error", self.rgb, gray)
        changed = save_pixels(
            self.root / "changed.png", (2, 2),
            [(255, 0, 0), (255, 0, 0), (0, 255, 0), (255, 255, 255)],
        )
        selection = SelectionQuery("color", {"color": "#000000", "tolerance": 0})
        with self.assertRaisesRegex(RenderDebugError, "different pixel coordinates"):
            self.pair("error", self.rgb, changed, selection=selection)

    def test_missing_and_unreadable_images_fail_explicitly(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "does not exist"):
            load_images([str(self.root / "missing.png")])
        path = self.root / "bad.png"
        path.write_text("not an image", encoding="utf-8")
        with self.assertRaisesRegex(RenderDebugError, "could not be decoded"):
            load_images([str(path)])

    def test_palette_and_one_bit_decoding_report_deterministic_conversion(self) -> None:
        palette_path = self.root / "palette.png"
        palette = Image.new("P", (1, 1), 0)
        palette.putpalette([255, 0, 0] + [0, 0, 0] * 255)
        palette.save(str(palette_path))
        palette.close()
        loaded = load_images([str(palette_path)])
        try:
            self.assertEqual("P", loaded[0].source_mode)
            self.assertEqual("RGB", loaded[0].working_mode)
            self.assertTrue(loaded[0].warnings)
        finally:
            loaded[0].image.close()

        bit_path = self.root / "bit.png"
        bit = Image.new("1", (1, 1), 1)
        bit.save(str(bit_path))
        bit.close()
        loaded = load_images([str(bit_path)])
        try:
            self.assertEqual("L", loaded[0].working_mode)
            self.assertEqual((255,), loaded[0].pixel(0, 0))
        finally:
            loaded[0].image.close()


if __name__ == "__main__":
    unittest.main()
