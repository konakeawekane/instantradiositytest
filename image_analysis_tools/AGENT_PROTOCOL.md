# Autonomous Graphics Analysis Protocol

Use this protocol when an agent develops graphics software and can inspect rendered image artifacts with `render-debug`. The goal is to make the tool routine for verification without turning uncertainty into unlimited exploratory queries.

The machine-readable policy is `agent_protocol.json`. If this guide and that file disagree, use the stricter query limit and update both in the same change.

## Activation

Use `render-debug` after a change to geometry/LOD, shaders, materials, lighting, shadows, cameras, projections, textures, samplers, asset import, post-processing, color management, compositing, alpha, or render targets when an output image exists.

Do not invoke it when the task cannot affect pixels, no artifact exists, or the only question is semantic object recognition. If a test already gives an exact visual assertion, skip exploration and run that assertion directly.

From the workspace root:

```bash
python -m image_analysis_tools.render_debug \
  --select all \
  --evaluate statistics \
  --images path/to/render.png \
  --compact
```

## Stage 1 — Baseline

For an unknown artifact, use no more than three queries:

1. `count` — dimensions, channels, and pixel count.
2. `statistics` — range, mean, median, and variation.
3. `dominant-colors --top 8` — broad palette and flat-color concentration.

Do not automatically add histograms, percentiles, grids, or individual samples. Summarize the baseline, then state one hypothesis.

## Stage 2 — Hypothesis

Before the first diagnostic query, write:

```text
Hypothesis: <one proposed explanation>
Prediction: <metric expected if true>
Rejection: <metric that would disprove it>
Query: <selection and evaluator>
```

Use no more than three hypothesis queries. Select the narrowest recipe that can change the implementation decision:

| Question | Preferred evaluators |
|---|---|
| Regression | `match`, then `error` or `channel-error` if it fails |
| Exposure/clipping | `percentiles`, `histogram`, `occupancy` |
| Color shift | `average-color`, `color-difference` |
| Alpha/compositing | `min-max`, `statistics`, targeted `occupancy` |
| Flat or low-detail region | regional `dominant-colors`, `statistics`, `color-count` |
| Spatial anomaly | regional `average-color`, `occupancy`, or `color-count` |
| Exact sample | `pixel-value` or `nearest-color` |
| Cubemap | face-center `pixel-value` plus inset regional `average-color` |

For spatial investigation, start with meaningful regions. At most one grid may be used per hypothesis, with no more than 16 cells. Narrow a suspicious region instead of repeatedly increasing the whole-image grid resolution.

## Stage 3 — Verification

After changing code, reuse the exact baseline selection and parameters. Use no more than two queries:

1. The primary pass/fail check, preferably `match` with a stated tolerance.
2. One diagnostic comparison such as `error`, `channel-error`, or `color-difference` only if the primary check fails or needs explanation.

Never resize, crop, or convert incompatible images merely to make a two-image evaluator run. When dimensions differ intentionally, compare the same normalized regions and distributions and state that the result is not pixel correspondence.

## Default budget and stopping

The default ceiling is eight queries per decision cycle:

- Baseline: 3
- Hypothesis: 3
- Verification: 2

An agent may exceed the ceiling only after recording why the next query can distinguish between remaining alternatives.

Stop immediately when any condition applies:

- The hypothesis is supported or rejected.
- The acceptance criterion passes or fails.
- Two successive queries do not reduce uncertainty.
- Another query would not change the next code change.
- The remaining question requires semantic visual understanding.
- The query ceiling is reached without an approved exception.

## Evidence discipline

Never equate a measurement with a diagnosis. Report:

```text
Observation: <exact value, selection, evaluator>
Inference: <what it suggests>
Confidence: <high|medium|low>
Alternatives: <other explanations consistent with the measurement>
Action: <code decision, stop, or one discriminating query>
Reproduction: <exact compact command>
```

Keep raw JSON out of the final narrative unless requested. Preserve the command and decisive values.

## Anti-patterns

- Do not run every evaluator because local compute is inexpensive; context and attention are still limited.
- Do not run `statistics`, `percentiles`, and `histogram` together unless each answers a different stated question.
- Do not use color distributions to identify objects or infer geometry without corroborating evidence.
- Do not keep subdividing an image after the remaining uncertainty becomes semantic.
- Do not declare success from a visually plausible render when an explicit baseline or threshold exists.

