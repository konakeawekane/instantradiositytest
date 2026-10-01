import math
import tempfile
import unittest
from pathlib import Path

from render_debug.errors import RenderDebugError
from render_debug.images import load_image
from render_debug.model import SelectionQuery
from render_debug.selection import resolve_selection, uv_to_pixel

from .helpers import save_pixels


class SelectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        path = save_pixels(
            self.root / "known.png", (2, 2),
            [(0, 0, 0, 0), (255, 0, 0, 128), (0, 255, 0, 255), (255, 0, 0, 255)],
            "RGBA",
        )
        self.image = load_image(str(path))

    def tearDown(self) -> None:
        self.image.image.close()
        self.temporary.cleanup()

    def test_registry_all_selects_every_pixel(self) -> None:
        result = resolve_selection(SelectionQuery("all"), self.image)
        self.assertEqual(4, result.pixel_count)
        self.assertEqual(((0, 0), (1, 0), (0, 1), (1, 1)), result.coordinate_tuple())
        self.assertEqual((2, 2), (result.image_width, result.image_height))
        self.assertEqual((0, 0, 0, 0), result.pixel_values[0])

    def test_pixel_is_zero_based_and_selects_one(self) -> None:
        result = resolve_selection(SelectionQuery("pixel", {"x": 1, "y": 0}), self.image)
        self.assertEqual((1, 0), result.point)
        self.assertEqual(1, result.pixel_count)

    def test_pixel_boundary_last_coordinate_is_valid(self) -> None:
        result = resolve_selection(SelectionQuery("pixel", {"x": 1, "y": 1}), self.image)
        self.assertEqual((1, 1), result.point)

    def test_pixel_rejects_out_of_bounds_and_non_integer(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "outside"):
            resolve_selection(SelectionQuery("pixel", {"x": 2, "y": 0}), self.image)
        with self.assertRaisesRegex(RenderDebugError, "integer"):
            resolve_selection(SelectionQuery("pixel", {"x": 0.5, "y": 0}), self.image)

    def test_uv_boundary_mapping(self) -> None:
        self.assertEqual((0, 0), uv_to_pixel(0.0, 0.0, 4, 3))
        self.assertEqual((3, 2), uv_to_pixel(1.0, 1.0, 4, 3))
        self.assertEqual((2, 1), uv_to_pixel(0.5, 0.5, 4, 3))

    def test_uv_rejects_out_of_range_and_non_finite(self) -> None:
        for value in (-0.1, 1.1, float("nan")):
            with self.assertRaisesRegex(RenderDebugError, "within"):
                uv_to_pixel(value, 0.5, 4, 3)

    def test_uv_rect_uses_pixel_center_inclusion(self) -> None:
        query = SelectionQuery("uv_rect", {"min": [0.25, 0.25], "max": [0.25, 0.75]})
        self.assertEqual(0, resolve_selection(query, self.image).pixel_count)
        query = SelectionQuery("uv_rect", {"min": [0.25, 0.25], "max": [0.75, 0.75]})
        self.assertEqual(4, resolve_selection(query, self.image).pixel_count)

    def test_uv_rect_full_boundary_selects_all(self) -> None:
        query = SelectionQuery("uv_rect", {"min": [0.0, 0.0], "max": [1.0, 1.0]})
        self.assertEqual(4, resolve_selection(query, self.image).pixel_count)

    def test_uv_rect_thin_region_can_be_empty(self) -> None:
        query = SelectionQuery("uv_rect", {"min": [0.0, 0.0], "max": [0.1, 0.1]})
        self.assertEqual(0, resolve_selection(query, self.image).pixel_count)

    def test_uv_rect_rejects_reversed_or_invalid_coordinates(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "must not exceed"):
            resolve_selection(SelectionQuery("uv_rect", {"min": [0.8, 0], "max": [0.2, 1]}), self.image)
        with self.assertRaisesRegex(RenderDebugError, "within"):
            resolve_selection(SelectionQuery("uv_rect", {"min": [-0.1, 0], "max": [1, 1]}), self.image)

    def test_pixel_rect_bounds_are_inclusive(self) -> None:
        query = SelectionQuery("pixel_rect", {"x_min": 0, "y_min": 0, "x_max": 1, "y_max": 1})
        result = resolve_selection(query, self.image)
        self.assertEqual(4, result.pixel_count)
        self.assertEqual((0, 0, 2, 2), result.bounds)

    def test_pixel_rect_single_coordinate_selects_one(self) -> None:
        query = SelectionQuery("pixel_rect", {"x_min": 1, "y_min": 1, "x_max": 1, "y_max": 1})
        self.assertEqual(((1, 1),), resolve_selection(query, self.image).coordinate_tuple())

    def test_pixel_rect_rejects_reversed_and_out_of_bounds(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "must not exceed"):
            resolve_selection(
                SelectionQuery("pixel_rect", {"x_min": 1, "y_min": 0, "x_max": 0, "y_max": 1}), self.image
            )
        with self.assertRaisesRegex(RenderDebugError, "inside"):
            resolve_selection(
                SelectionQuery("pixel_rect", {"x_min": 0, "y_min": 0, "x_max": 2, "y_max": 1}), self.image
            )

    def test_uv_radius_uses_pixel_centers(self) -> None:
        query = SelectionQuery("uv_radius", {"u": 0.25, "v": 0.25, "radius": 0.0})
        self.assertEqual(((0, 0),), resolve_selection(query, self.image).coordinate_tuple())
        query = SelectionQuery("uv_radius", {"u": 0.5, "v": 0.5, "radius": math.sqrt(0.125)})
        self.assertEqual(4, resolve_selection(query, self.image).pixel_count)

    def test_uv_radius_can_be_empty_and_rejects_negative(self) -> None:
        query = SelectionQuery("uv_radius", {"u": 0.0, "v": 0.0, "radius": 0.0})
        self.assertEqual(0, resolve_selection(query, self.image).pixel_count)
        with self.assertRaisesRegex(RenderDebugError, "non-negative"):
            resolve_selection(SelectionQuery("uv_radius", {"u": 0, "v": 0, "radius": -0.1}), self.image)

    def test_color_rgb_target_ignores_alpha(self) -> None:
        query = SelectionQuery("color", {"color": "#FF0000", "tolerance": 0.0})
        self.assertEqual(((1, 0), (1, 1)), resolve_selection(query, self.image).coordinate_tuple())

    def test_color_rgba_target_includes_alpha(self) -> None:
        query = SelectionQuery("color", {"color": "#FF000080", "tolerance": 0.0})
        self.assertEqual(((1, 0),), resolve_selection(query, self.image).coordinate_tuple())

    def test_color_tolerance_uses_shared_rms_distance(self) -> None:
        query = SelectionQuery("color", {"color": "#FE0000", "tolerance": 0.003})
        self.assertEqual(2, resolve_selection(query, self.image).pixel_count)

    def test_color_rejects_invalid_hex_and_tolerance(self) -> None:
        with self.assertRaisesRegex(RenderDebugError, "hexadecimal"):
            resolve_selection(SelectionQuery("color", {"color": "red", "tolerance": 0}), self.image)
        with self.assertRaisesRegex(RenderDebugError, "within"):
            resolve_selection(SelectionQuery("color", {"color": "#FF0000", "tolerance": 2}), self.image)

    def test_predicate_supports_boolean_precedence_and_parentheses(self) -> None:
        query = SelectionQuery("predicate", {"expression": "R > 0.9 AND (A < 0.75 OR G > 0.5)"})
        self.assertEqual(((1, 0),), resolve_selection(query, self.image).coordinate_tuple())

    def test_predicate_boundary_comparisons(self) -> None:
        query = SelectionQuery("predicate", {"expression": "R >= 1 AND A == 1"})
        self.assertEqual(((1, 1),), resolve_selection(query, self.image).coordinate_tuple())

    def test_predicate_can_select_zero_and_rejects_invalid_expression(self) -> None:
        query = SelectionQuery("predicate", {"expression": "R > 2"})
        self.assertEqual(0, resolve_selection(query, self.image).pixel_count)
        with self.assertRaises(RenderDebugError):
            resolve_selection(SelectionQuery("predicate", {"expression": "R + 1"}), self.image)


if __name__ == "__main__":
    unittest.main()
