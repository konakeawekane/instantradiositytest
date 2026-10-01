# Render Debug Tools

## Purpose

`render_debug` is a project-independent image analysis toolset intended to give an AI coding agent precise information about rendered images that is difficult or unreliable to obtain from visual inspection alone.

The toolset does **not** capture data from the graphics pipeline. It operates on image files produced by the application.

The primary use case is iterative rendering/debugging with Codex:

1. Codex identifies something it wants to know about one or more images.
2. Codex issues a query through the `render-debug` CLI.
3. The tool performs precise numerical analysis.
4. The tool returns a concise, machine-readable result that Codex can use to form or test a hypothesis.

## Design Principles

- **Image-focused:** The toolset analyzes image files only. It does not depend on a particular graphics API, renderer, engine, or project architecture.
- **Query-driven:** The AI asks for specific information rather than receiving a large predefined report every time.
- **Small interface:** All queries pass through one CLI. There is no need for separate user-facing commands or modules for each individual analysis.
- **Composable:** Pixel selection and evaluation are separate concepts so new analyses can reuse existing selection methods.
- **Precise:** Return exact numerical data rather than relying on visual estimation.
- **Deterministic:** Identical inputs and parameters should produce identical results.
- **Agent-friendly:** Results should be easy for Codex to parse and reason about.
- **Incremental:** Start with a small set of high-value capabilities and expand based on real debugging needs.

---

# CLI Query Model

Every query consists of three main components:

1. **Selection** — what pixels are being considered and the parameters defining that selection.
2. **Evaluation** — what analysis should be performed on the selected pixels and its parameters.
3. **Images** — the image file or files relevant to the evaluation.

Conceptually:

```text
SELECT <selection type> <selection parameters>
EVALUATE <evaluation type> <evaluation parameters>
IMAGES <image references>
```

The actual CLI syntax should preserve this conceptual structure.

A normalized query should internally look approximately like:

```json
{
  "selection": {
    "type": "uv_rect",
    "parameters": {
      "min": [0.3, 0.4],
      "max": [0.5, 0.8]
    }
  },
  "evaluation": {
    "type": "average_color",
    "parameters": {}
  },
  "images": [
    "image.png"
  ]
}
```

The CLI parser should convert command-line arguments into this normalized query representation before evaluation.

## Example Queries

### Average color in a UV region

```bash
render-debug \
  --select uv-rect 0.3 0.4 0.5 0.8 \
  --evaluate average-color \
  --images image.png
```

### Compare two images in a UV region

```bash
render-debug \
  --select uv-rect 0.3 0.4 0.5 0.8 \
  --evaluate error \
  --images reference.png actual.png
```

### Sample a pixel using UV coordinates

```bash
render-debug \
  --select uv 0.8 0.4 \
  --evaluate pixel-value \
  --images image.png
```

### Sample a pixel using image coordinates

```bash
render-debug \
  --select pixel 421 237 \
  --evaluate pixel-value \
  --images image.png
```

---

# Selection System

Selection defines the set of pixels passed to an evaluation.

The initial implementation should intentionally support only a small set of selection types.

## `all`

Select the entire image.

```text
select: all
```

No parameters.

## `uv-rect`

Select a rectangular region using normalized UV coordinates.

```text
select: uv-rect <u_min> <v_min> <u_max> <v_max>
```

Example:

```text
uv-rect 0.3 0.4 0.5 0.8
```

Initial coordinate convention:

```text
(0, 0) = top-left
(1, 1) = bottom-right
```

The implementation should document and enforce boundary behavior consistently.

## `uv`

Select the pixel corresponding to a normalized UV coordinate.

```text
select: uv <u> <v>
```

This is intended primarily for pixel sampling.

## `pixel`

Select a single pixel using integer image coordinates.

```text
select: pixel <x> <y>
```

The implementation should explicitly define whether coordinates are zero-based. Zero-based coordinates are recommended.

## Future Selection Types

Do not implement these initially, but keep the selection system extensible enough that they could be added later:

- pixel rectangle
- circular region
- arbitrary mask
- predicate-based pixel selection
- comparison-derived selection

The selection system should not assume that every future selector is rectangular or UV-based.

---

# Evaluation System

Evaluation defines what information should be extracted from the selected pixels.

The first implementation should focus on a very small number of high-value analyses.

## `pixel-value`

Return the value of a selected pixel.

Supported selections initially:

- `uv`
- `pixel`

For PNG images, return the pixel value in hexadecimal and expose the underlying channel values when useful.

For uncompressed/floating-point image formats, return the individual channel values.

Example conceptual result for PNG:

```text
Pixel
  XY: 421, 237
  UV: 0.2193, 0.2194
  Hex: #7FA342
```

Example conceptual result for floating-point data:

```text
Pixel
  XY: 421, 237
  UV: 0.2193, 0.2194
  R: 0.498
  G: 0.639
  B: 0.259
  A: 1.000
```

## `average-color`

Calculate the average color of all selected pixels.

Example:

```text
average-color image.png --region 0.3,0.4,0.5,0.8
```

The result should include numerical channel values and a convenient display representation where appropriate.

For PNG:

```text
Average color
  RGB: 128.42, 93.17, 64.81
  Hex: #805D41
```

For floating-point images:

```text
Average color
  R: 0.502
  G: 0.365
  B: 0.254
  A: 1.000
```

## `error`

Compare the selected pixels from two images.

Initial scope should focus on raw channel-wise absolute error and associated aggregate statistics rather than a single opaque similarity score.

At minimum, the result should expose:

- selected pixel count
- total absolute error per channel
- mean absolute error per channel
- maximum channel error
- overall aggregate error where clearly defined

Example conceptual result:

```text
Comparison
  Pixels: 384 × 432 = 165,888

Absolute error
  R total: 1,812.4
  G total:   824.7
  B total: 3,051.2

Mean absolute error
  R: 0.0109
  G: 0.0050
  B: 0.0184

Maximum channel error: 0.74
```

The mathematical definition of every reported metric must be explicit and covered by tests.

---

# Planned Color Analyses

These are part of the intended near-term feature set, but should be added incrementally after the initial three evaluations above are working.

## `dominant-colors`

Return the top `N` distinct colors within the selected region.

Example:

```text
--evaluate dominant-colors --top 8
```

Conceptual result:

```text
1. #384A62 — 24.1%
2. #3B4D65 — 18.7%
3. #414F63 — 11.2%
...
```

The implementation will need an explicit definition of what makes colors "distinct" so that small pixel-level variations do not create an enormous number of separate colors.

## `color-distribution`

Return sampled points along the image's color/channel distribution.

The AI should be able to request a number of samples, e.g.:

```text
--evaluate color-distribution --samples 100
```

The intended use is similar to reading an image-editing curve: the AI can inspect percentile positions and the corresponding channel values.

The implementation should define exactly what each percentile represents and should preserve enough information to compare distributions between images later.

## `nearest-color`

Find the pixel whose color is nearest to a requested target color, optionally within the selected region.

Example:

```text
--evaluate nearest-color --color "#FF0000"
```

Potential result:

```text
Nearest color
  RGB: 251, 3, 2
  Hex: #FB0302
  XY: 823, 417
  UV: 0.4286, 0.3861
  Distance: 0.017
```

The color-distance metric must be explicitly defined when this is implemented.

---

# Reporting

The tool should return results in a form designed for direct consumption by Codex.

Reporting is primarily about **structured analytical output**, not generating elaborate visual reports.

## Canonical Result

The internal result should be serializable as JSON.

A result should identify:

- query type
- selection type and parameters
- evaluation type and parameters
- image references
- image dimensions where relevant
- numerical result values
- units or interpretation where needed
- warnings/errors

Example:

```json
{
  "selection": {
    "type": "uv_rect",
    "parameters": {
      "min": [0.3, 0.4],
      "max": [0.5, 0.8]
    }
  },
  "evaluation": {
    "type": "average_color",
    "parameters": {}
  },
  "images": ["image.png"],
  "result": {
    "rgb": [128.42, 93.17, 64.81],
    "hex": "#805D41"
  }
}
```

## Human-Readable Output

The CLI should also produce concise terminal output by default so the same query is useful during interactive development.

The JSON schema should remain the authoritative representation of the result.

## Errors and Warnings

The tool should fail explicitly for invalid queries, including:

- missing image files
- unreadable or unsupported images
- incompatible image dimensions for comparisons
- invalid UV coordinates
- invalid pixel coordinates
- missing required evaluation parameters
- incorrect number of image references

Warnings should be used for conditions that do not make the query invalid but could affect interpretation.

---

# Image Handling

The initial toolset should be image-format agnostic at the query level, while keeping format-specific interpretation where required.

Important considerations:

- image dimensions
- channel count
- bit depth
- integer vs floating-point channels
- alpha presence
- color-space metadata where available
- PNG channel representation
- uncompressed/floating-point channel representation

Format-specific behavior must be explicit in results when it materially affects interpretation.

---

# High-Level Project Structure

The implementation can remain simple and does **not** need one module per CLI operation.

```text
render_debug/
├── AGENTS.md
├── README.md
├── cli.py
├── analyze/
├── compare/
├── report/
└── tests/
```

The exact internal Python module structure should be derived from the shared query/evaluation model rather than from a one-to-one mapping of CLI commands.

A likely conceptual flow is:

```text
CLI arguments
      │
      ▼
 Query parser
      │
      ▼
 Normalized query
      │
      ├───────────────┐
      ▼               ▼
 Selection        Evaluation
      │               │
      └───────┬───────┘
              ▼
        Image analysis
              │
              ▼
          Result object
              │
        ┌─────┴─────┐
        ▼           ▼
      JSON        CLI text
```

---

# Tests

The tests validate the correctness of the **image-analysis tool itself**, not the graphics application.

The initial tests should establish confidence in:

- UV-to-pixel coordinate conversion
- pixel selection boundaries
- pixel-value extraction
- average-color calculations
- image comparison math
- channel handling
- invalid input handling

Use deterministic fixture images designed to produce mathematically known results.

Example structure:

```text
tests/
├── test_selection.py
├── test_pixel_value.py
├── test_average_color.py
├── test_compare.py
└── fixtures/
    ├── black.png
    ├── white.png
    ├── known_colors.png
    └── known_pattern.png
```

As real rendering-debugging cases expose shortcomings in the tool, those cases can become additional regression tests.

---

# Initial Implementation Scope

The first working version should contain only:

### Selection

- `all`
- `uv`
- `pixel`
- `uv-rect`

### Evaluation

- `pixel-value`
- `average-color`
- `error`

### Reporting

- structured JSON result
- concise terminal output
- explicit validation/errors

The next additions should be driven by real rendering-debugging sessions, with likely candidates being:

- `dominant-colors`
- `color-distribution`
- `nearest-color`

Do not implement geometry inference, pipeline inspection, capture systems, or elaborate visualization infrastructure as part of this toolset.

---

# Design Goal

The long-term goal is to let Codex ask questions such as:

> What is the average color in this UV region?

> What is the exact pixel value at this UV coordinate?

> How much do these two images differ in this region, and which channel accounts for the difference?

> What are the eight most prominent distinct colors in this region?

> Where in the image is the nearest occurrence of this target color?

The tool should answer these questions with precise, reproducible data so that the AI can use the results to form hypotheses, investigate rendering bugs, and verify changes.
