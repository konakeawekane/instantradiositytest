import unittest

from render_debug.errors import RenderDebugError
from render_debug.predicate import evaluate_predicate, parse_predicate


class PredicateTests(unittest.TestCase):
    def test_and_has_higher_precedence_than_or(self) -> None:
        node = parse_predicate("R > 0.5 OR G > 0.5 AND B < 0.2")
        self.assertTrue(evaluate_predicate(node, {"R": 0, "G": 1, "B": 0, "A": 1}))
        self.assertFalse(evaluate_predicate(node, {"R": 0, "G": 1, "B": 1, "A": 1}))

    def test_parentheses_override_precedence(self) -> None:
        node = parse_predicate("(R > 0.5 OR G > 0.5) AND B < 0.2")
        self.assertFalse(evaluate_predicate(node, {"R": 1, "G": 0, "B": 1, "A": 1}))

    def test_all_comparison_operators(self) -> None:
        channels = {"R": 0.5, "G": 0.5, "B": 0.5, "A": 1.0}
        for expression in ("R > 0.4", "R >= 0.5", "R < 0.6", "R <= 0.5", "R == 0.5", "R != 0.4"):
            self.assertTrue(evaluate_predicate(parse_predicate(expression), channels), expression)

    def test_parser_is_case_insensitive_and_accepts_trailing_space(self) -> None:
        node = parse_predicate("r > 0.1 and a == 1   ")
        self.assertTrue(evaluate_predicate(node, {"R": 0.2, "G": 0, "B": 0, "A": 1}))

    def test_parser_accepts_signed_and_exponent_values(self) -> None:
        node = parse_predicate("R > -2e-1 AND G <= +0.5")
        self.assertTrue(evaluate_predicate(node, {"R": 0, "G": 0.5, "B": 0, "A": 1}))

    def test_invalid_syntax_fails_explicitly(self) -> None:
        for expression in ("", "X > 0", "R + 1", "R >", "(R > 0"):
            with self.assertRaises(RenderDebugError, msg=expression):
                parse_predicate(expression)


if __name__ == "__main__":
    unittest.main()
