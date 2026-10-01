from dataclasses import dataclass, field
from typing import Any, Dict, List


@dataclass(frozen=True)
class SelectionQuery:
    type: str
    parameters: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return {"type": self.type.replace("_", "-"), "parameters": self.parameters}


@dataclass(frozen=True)
class EvaluationQuery:
    type: str
    parameters: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return {"type": self.type.replace("_", "-"), "parameters": self.parameters}


@dataclass(frozen=True)
class Query:
    selection: SelectionQuery
    evaluation: EvaluationQuery
    images: List[str]

    def to_dict(self) -> Dict[str, Any]:
        return {
            "selection": self.selection.to_dict(),
            "evaluation": self.evaluation.to_dict(),
            "images": self.images,
        }
