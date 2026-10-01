import json
import unittest
from pathlib import Path

from render_debug.cli import EVALUATORS


class AgentProtocolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        path = Path(__file__).resolve().parents[1] / "agent_protocol.json"
        cls.protocol = json.loads(path.read_text(encoding="utf-8"))

    def test_protocol_has_version_and_three_ordered_stages(self) -> None:
        self.assertEqual("1.0", self.protocol["schema_version"])
        self.assertEqual(
            ["baseline", "hypothesis", "verification"],
            [stage["id"] for stage in self.protocol["stages"]],
        )

    def test_stage_budgets_equal_hard_limit(self) -> None:
        budgets = self.protocol["budgets"]
        total = (
            budgets["baseline_max_queries"]
            + budgets["hypothesis_max_queries"]
            + budgets["verification_max_queries"]
        )
        self.assertEqual(budgets["default_hard_query_limit"], total)
        self.assertEqual(
            [stage["maximum_queries"] for stage in self.protocol["stages"]],
            [
                budgets["baseline_max_queries"],
                budgets["hypothesis_max_queries"],
                budgets["verification_max_queries"],
            ],
        )

    def test_every_recommended_evaluator_exists(self) -> None:
        valid = set(EVALUATORS)
        recommended = set()
        for stage in self.protocol["stages"]:
            recommended.update(stage.get("default_evaluators", []))
            recommended.update(stage.get("preferred_evaluators", []))
        for recipe in self.protocol["recipes"].values():
            recommended.update(recipe["evaluators"])
        self.assertEqual(set(), recommended - valid)

    def test_recipe_limits_fit_hypothesis_budget(self) -> None:
        maximum = self.protocol["budgets"]["hypothesis_max_queries"]
        for name, recipe in self.protocol["recipes"].items():
            self.assertGreater(recipe["query_limit"], 0, name)
            self.assertLessEqual(recipe["query_limit"], maximum, name)

    def test_spatial_grid_policy_is_bounded(self) -> None:
        budgets = self.protocol["budgets"]
        self.assertLessEqual(budgets["maximum_spatial_grid_cells"], 16)
        self.assertEqual(1, budgets["maximum_spatial_grids_per_hypothesis"])

    def test_reporting_contract_has_decision_fields(self) -> None:
        fields = set(self.protocol["report_fields"])
        self.assertTrue({"observation", "inference", "confidence", "action"} <= fields)


if __name__ == "__main__":
    unittest.main()

