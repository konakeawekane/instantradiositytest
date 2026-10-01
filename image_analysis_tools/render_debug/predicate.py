import re
from dataclasses import dataclass
from typing import Dict, List, Sequence, Tuple, Union

from .errors import RenderDebugError


TOKEN = re.compile(
    r"\s*(>=|<=|==|!=|>|<|\(|\)|AND\b|OR\b|[RGBA]\b|[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)",
    re.IGNORECASE,
)

ComparisonNode = Tuple[str, str, float]
BooleanNode = Tuple[str, "PredicateNode", "PredicateNode"]
PredicateNode = Union[ComparisonNode, BooleanNode]


def _tokenize(expression: str) -> List[str]:
    tokens: List[str] = []
    position = 0
    while position < len(expression):
        if expression[position:].strip() == "":
            break
        match = TOKEN.match(expression, position)
        if not match:
            raise RenderDebugError(
                "invalid_predicate", "Invalid token in predicate",
                {"position": position, "expression": expression},
            )
        tokens.append(match.group(1).upper())
        position = match.end()
    if not tokens:
        raise RenderDebugError("invalid_predicate", "Predicate must not be empty")
    return tokens


@dataclass
class _Parser:
    tokens: Sequence[str]
    position: int = 0

    def peek(self) -> str:
        return self.tokens[self.position] if self.position < len(self.tokens) else ""

    def take(self) -> str:
        token = self.peek()
        if not token:
            raise RenderDebugError("invalid_predicate", "Unexpected end of predicate")
        self.position += 1
        return token

    def parse(self) -> PredicateNode:
        node = self.parse_or()
        if self.peek():
            raise RenderDebugError("invalid_predicate", "Unexpected predicate token", {"token": self.peek()})
        return node

    def parse_or(self) -> PredicateNode:
        node = self.parse_and()
        while self.peek() == "OR":
            self.take()
            node = ("OR", node, self.parse_and())
        return node

    def parse_and(self) -> PredicateNode:
        node = self.parse_factor()
        while self.peek() == "AND":
            self.take()
            node = ("AND", node, self.parse_factor())
        return node

    def parse_factor(self) -> PredicateNode:
        if self.peek() == "(":
            self.take()
            node = self.parse_or()
            if self.take() != ")":
                raise RenderDebugError("invalid_predicate", "Expected closing parenthesis")
            return node
        channel = self.take()
        if channel not in {"R", "G", "B", "A"}:
            raise RenderDebugError("invalid_predicate", "Expected channel R, G, B, or A", {"token": channel})
        operator = self.take()
        if operator not in {">", ">=", "<", "<=", "==", "!="}:
            raise RenderDebugError("invalid_predicate", "Expected comparison operator", {"token": operator})
        raw_value = self.take()
        try:
            value = float(raw_value)
        except ValueError:
            raise RenderDebugError("invalid_predicate", "Expected numeric comparison value", {"token": raw_value})
        return channel, operator, value


def parse_predicate(expression: str) -> PredicateNode:
    if not isinstance(expression, str):
        raise RenderDebugError("invalid_predicate", "Predicate must be a string")
    return _Parser(_tokenize(expression)).parse()


def evaluate_predicate(node: PredicateNode, channels: Dict[str, float]) -> bool:
    if node[0] == "AND":
        return evaluate_predicate(node[1], channels) and evaluate_predicate(node[2], channels)
    if node[0] == "OR":
        return evaluate_predicate(node[1], channels) or evaluate_predicate(node[2], channels)
    channel, operator, value = node
    actual = channels[channel]
    operations = {
        ">": actual > value,
        ">=": actual >= value,
        "<": actual < value,
        "<=": actual <= value,
        "==": actual == value,
        "!=": actual != value,
    }
    return operations[operator]
