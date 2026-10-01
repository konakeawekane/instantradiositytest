# AGENTS.md

Render Debug Tools v1 is a deterministic, JSON-first Python image-analysis CLI intended for direct use by coding agents.

## Scope and stack

- Python 3.8+ and Pillow; package metadata and the `render-debug` entry point live in `pyproject.toml`.
- `render_debug/` contains the CLI, normalized query model, selection resolution, image loading, and evaluations.
- `tests/` contains deterministic mathematical checks built with `unittest`.
- `image_analysis_tools_specification.md` is the v1 contract, `render_debug_tools.md` is the original design, and `README.md` documents the implemented interface and exact math.
- `AGENT_PROTOCOL.md` and `agent_protocol.json` define when autonomous agents should query the tool, their query budgets, and stop conditions.

## Commands

- Install editable: `python -m pip install -e .`
- Run without installation: `python -m render_debug --select all --evaluate average-color --images <image>`
- Test: `python -m unittest discover -s tests -t . -v`
- Syntax check: `python -m compileall -q render_debug tests`
- Build: `python -m build` (requires the optional `build` package)
- Lint: none configured

## Rules

- Keep stdout valid JSON on success. Emit valid JSON errors to stderr and return nonzero; never mix logs or prose into the JSON stream.
- Keep selection independent from evaluation. The public registries must remain exactly the eight selectors and eighteen evaluators in the v1 specification.
- Make coordinate boundaries, channel units, color-space assumptions, and every aggregate formula explicit in both results and `README.md`.
- Preserve deterministic behavior: no sampling, randomized quantization, timestamps, or environment-dependent path ordering unless the query explicitly requests it.
- Add mathematically known tests for every metric and boundary. Prefer tiny generated fixtures whose expected values can be checked by hand.
- When investigating rendered output autonomously, follow `AGENT_PROTOCOL.md`; do not exceed its default query budget without recording the discriminating reason.

## Guardrails

- Do not add geometry inference, graphics-pipeline capture, or visualization generation; this package analyzes image files only.
- Do not hide incompatible dimensions or channels behind implicit resizing/conversion. Reject the comparison with a structured error.
- Do not round analytical values for presentation. Keep full JSON numeric precision and add separate display fields such as hex when useful.
- Do not add selectors or evaluators outside the v1 registries without a new specification; extend shared color, predicate, or metric definitions instead of creating inconsistent local variants.
