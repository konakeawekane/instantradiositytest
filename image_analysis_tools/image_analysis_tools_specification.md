# Render Debug Tool — Version 1 Implementation Specification

## 1. Scope

Implement a project-independent image interrogation CLI for rendered image artifacts.

The tool:

* Reads image files from disk.
* Selects pixels using one V1 selector.
* Evaluates numerical properties using one V1 evaluator.
* Supports single-image and two-image evaluations.
* Returns deterministic, machine-readable results.
* Does not inspect renderer state, scene state, geometry, materials, shaders, or graphics-pipeline data.

The CLI is the interface exposed to the calling agent.

V1 contains:

* 8 selectors
* 18 evaluators

---

## 2. CLI

Executable:

```text
render-debug
```

Every query contains:

```text
render-debug \
  --select <selector> [selector parameters] \
  --evaluate <evaluator> [evaluator parameters] \
  --images <image> [<image> ...]
```

All three components are required.

### Selection

```text
--select <selector>
```

### Evaluation

```text
--evaluate <evaluator>
```

### Images

```text
--images <path> [<path> ...]
```

The evaluator determines the required number of images.

---

## 3. Normalized Query

Parse every CLI invocation into an internal representation equivalent to:

```json
{
  "selection": {
    "type": "uv-rect",
    "parameters": {
      "min": [0.3, 0.4],
      "max": [0.5, 0.8]
    }
  },
  "evaluation": {
    "type": "average-color",
    "parameters": {}
  },
  "images": [
    "image.png"
  ]
}
```

Selection and evaluation must remain independent.

The selector produces a pixel selection. The evaluator operates on that selection.

---

# 4. Coordinate Conventions

## 4.1 Pixel coordinates

Pixel coordinates are integer coordinates:

```text
x = 0 ... width - 1
y = 0 ... height - 1
```

Origin:

```text
(0, 0) = top-left
```

## 4.2 UV coordinates

UV coordinates are normalized floating-point coordinates:

```text
u ∈ [0, 1]
v ∈ [0, 1]
```

Convention:

```text
(0, 0) = top-left
(1, 1) = bottom-right
```

The implementation must use one consistent UV-to-pixel mapping for all UV selectors.

The mapping must be covered by boundary tests.

---

# 5. Image Loading

The implementation must provide a consistent internal pixel representation to selectors and evaluators.

At minimum support the image formats supported by the existing implementation, including PNG and applicable uncompressed formats.

Preserve:

* width
* height
* channel count
* channel values
* alpha when present
* source numeric representation where applicable

Normalized integer image channels may be represented internally as floating-point values in `[0, 1]`.

Floating-point source values must not be unnecessarily quantized.

Alpha must not be silently discarded.

---

# 6. V1 Selectors

## 6.1 `all`

Select every pixel.

Parameters:

```text
none
```

Example:

```text
--select all
```

---

## 6.2 `pixel`

Select one pixel using integer XY coordinates.

Parameters:

```text
x y
```

Example:

```text
--select pixel 128 64
```

Requirements:

* `x` and `y` must be integers.
* Coordinates must be inside the image.
* Exactly one pixel is selected.

---

## 6.3 `uv`

Select one pixel using normalized UV coordinates.

Parameters:

```text
u v
```

Example:

```text
--select uv 0.8 0.4
```

Requirements:

* `u` and `v` must be in `[0, 1]`.
* The coordinate must map deterministically to one pixel.
* Exactly one pixel is selected.

---

## 6.4 `uv-rect`

Select pixels whose pixel centers fall within a normalized UV rectangle.

Parameters:

```text
u_min v_min u_max v_max
```

Example:

```text
--select uv-rect 0.3 0.4 0.5 0.8
```

Requirements:

* All coordinates must be in `[0, 1]`.
* Minimum coordinates must not exceed maximum coordinates.
* Selection is rectangular.
* Boundary behavior must be deterministic and tested.

---

## 6.5 `pixel-rect`

Select pixels in an integer pixel rectangle.

Parameters:

```text
x_min y_min x_max y_max
```

Example:

```text
--select pixel-rect 100 50 300 200
```

Requirements:

* Coordinates must be integers.
* Coordinates must be inside the image.
* Minimum coordinates must not exceed maximum coordinates.
* Rectangle bounds are inclusive.

---

## 6.6 `uv-radius`

Select pixels within a circular radius of a UV coordinate.

Parameters:

```text
u v radius
```

Example:

```text
--select uv-radius 0.8 0.4 0.05
```

Requirements:

* `u` and `v` must be in `[0, 1]`.
* Radius must be non-negative.
* Distance is measured in normalized UV space.
* Pixel-center positions are used for inclusion.

---

## 6.7 `color`

Select pixels matching or sufficiently close to a specified color.

Parameters:

```text
color [tolerance]
```

Examples:

```text
--select color #FF0000
--select color #FF0000 0.02
```

Requirements:

* Hexadecimal RGB/RGBA colors are supported.
* Zero tolerance means exact matching.
* Non-zero tolerance permits colors within the specified color distance.
* Matching must be deterministic.
* RGB matching must not silently incorporate alpha.

The color-distance definition must be shared with `color-count`.

---

## 6.8 `predicate`

Select pixels satisfying a channel/value expression.

Example:

```text
--select predicate "R > 0.8 AND B < 0.2"
```

Supported channels:

```text
R
G
B
A
```

Supported comparisons:

```text
>
>=
<
<=
==
!=
```

Supported boolean operators:

```text
AND
OR
```

Parentheses should be supported.

Channel values are normalized values unless the source representation explicitly requires another representation.

Invalid expressions must produce an error.

---

# 7. Selection Result

A selector produces an internal selection containing at least:

```text
image width
image height
selected pixel coordinates
selected pixel values
selected pixel count
```

Selectors must not perform evaluator-specific calculations.

A selection containing zero pixels is valid.

Each evaluator must define its behavior for empty selections.

---

# 8. V1 Evaluators

## 8.1 `pixel-value`

Return the exact value of the selected pixel.

Intended for `pixel` and `uv`.

For PNG RGB/RGBA images, return hexadecimal color values:

```text
#7FA342
#7FA342FF
```

For floating-point/uncompressed formats, return explicit channels:

```json
{
  "r": 0.498,
  "g": 0.639,
  "b": 0.259,
  "a": 1.0
}
```

Reject the query if more than one pixel is selected.

---

## 8.2 `average-color`

Calculate the arithmetic mean of every available channel.

```text
mean(C) = sum(C_i) / N
```

Return per-channel values.

Example:

```json
{
  "r": 0.42,
  "g": 0.51,
  "b": 0.37,
  "a": 1.0
}
```

---

## 8.3 `min-max`

Return the minimum and maximum value for every available channel.

Example:

```json
{
  "r": {
    "min": 0.12,
    "max": 0.91
  },
  "g": {
    "min": 0.08,
    "max": 0.74
  },
  "b": {
    "min": 0.03,
    "max": 0.66
  }
}
```

---

## 8.4 `percentiles`

Calculate requested channel percentiles.

Parameters:

```text
--percentiles <p1,p2,...>
```

Example:

```text
--evaluate percentiles --percentiles 1,25,50,75,99
```

Requirements:

* Percentiles must be in `[0, 100]`.
* Return every requested percentile for every available channel.
* Use one deterministic percentile interpolation method.
* The interpolation method must be covered by tests.

---

## 8.5 `statistics`

Return descriptive statistics for every available channel.

Required values:

```text
count
mean
median
variance
standard_deviation
minimum
maximum
```

---

## 8.6 `dominant-colors`

Return the most prevalent distinct colors.

Parameters:

```text
--top <N>
```

Example:

```text
--evaluate dominant-colors --top 8
```

Each result contains:

```text
color
pixel count
percentage
```

Near-identical colors must be grouped using a deterministic color-distance/clustering rule.

Results are ordered by descending prevalence.

Ties must use a deterministic ordering.

---

## 8.7 `color-distribution`

Return sampled color/channel distributions.

Parameters:

```text
--samples <N>
```

Example:

```text
--evaluate color-distribution --samples 100
```

Return ordered percentile samples for each available channel.

Each sample contains:

```text
percentile
value
```

For `N` samples, return exactly `N` samples per channel for non-empty selections.

The sampling rule must be deterministic.

---

## 8.8 `nearest-color`

Find the selected pixel whose color is nearest to a target color.

Parameters:

```text
--color <hex-color>
```

Example:

```text
--evaluate nearest-color --color #FF0000
```

Return:

```text
pixel coordinate
UV coordinate
pixel color
distance to target
```

If multiple pixels have equal minimum distance, use deterministic pixel ordering as the tie-break.

---

## 8.9 `color-count`

Count pixels matching or approximately matching a target color.

Parameters:

```text
--color <hex-color>
--tolerance <value>
```

Return:

```text
count
percentage
```

Percentage is relative to the selected pixel count.

The color-distance definition must be identical to the `color` selector.

---

## 8.10 `color-range`

Return the per-channel range represented by the selected pixels.

For each channel return:

```text
minimum
maximum
range
```

where:

```text
range = maximum - minimum
```

---

## 8.11 `histogram`

Return fixed-bin channel distributions.

Parameters:

```text
--bins <N>
```

Example:

```text
--evaluate histogram --bins 256
```

For each available channel return:

```text
bin lower bound
bin upper bound
count
percentage
```

For normalized channels the default range is:

```text
[0, 1]
```

The binning convention must be deterministic.

---

## 8.12 `occupancy`

Calculate the fraction of selected pixels satisfying a condition.

Parameters:

```text
--condition <predicate>
```

Example:

```text
--evaluate occupancy --condition "R > 0.8 AND B < 0.2"
```

Return:

```text
count
total
fraction
percentage
```

The condition operates on the already-selected pixel set.

---

## 8.13 `count`

Return the number of selected pixels.

Example result:

```json
{
  "count": 12345
}
```

---

## 8.14 `error`

Compare corresponding selected pixels between two images.

Required images:

```text
2
```

Requirements:

1. Load both images.
2. Validate compatible dimensions.
3. Apply the same selector to both.
4. Verify corresponding selections.
5. Compare corresponding pixel channels.
6. Return aggregate error.

Return at minimum:

```text
pixel_count
channel_count
total_error
per-channel aggregate error
```

The exact error metric must be deterministic and documented in the implementation.

---

## 8.15 `error-distribution`

Return the distribution of corresponding pixel errors between two images.

Required images:

```text
2
```

Parameters:

```text
--bins <N>
```

Return:

```text
bin
count
percentage
```

The error metric must be identical to `error`.

---

## 8.16 `channel-error`

Return image-to-image error independently for each channel.

Required images:

```text
2
```

Return per-channel aggregate and mean errors.

Example:

```json
{
  "r": {
    "total": 12.4,
    "mean": 0.0021
  },
  "g": {
    "total": 8.1,
    "mean": 0.0014
  },
  "b": {
    "total": 3.7,
    "mean": 0.0006
  }
}
```

Only channels present in the source images are returned.

---

## 8.17 `color-difference`

Compare the color populations of selected regions in two images.

Required images:

```text
2
```

Return numerical differences between the two selected color populations.

At minimum include:

```text
per-channel mean difference
per-channel distribution difference
```

The result must identify the metric used.

The metric must be deterministic.

---

## 8.18 `match`

Determine whether two selected regions satisfy an equivalence tolerance.

Required images:

```text
2
```

Parameters:

```text
--tolerance <value>
```

Example result:

```json
{
  "match": true,
  "tolerance": 0.001,
  "max_error": 0.0004
}
```

The comparison must operate on corresponding pixels.

The result must be deterministic.

---

# 9. Multi-Image Evaluation

Two-image evaluators require exactly two images.

For two-image evaluation:

1. Load both images.
2. Validate dimensions.
3. Apply the same selector to each image.
4. Verify that the selections correspond.
5. Compare corresponding pixels.
6. Produce the evaluator result.

Do not:

* resize images
* crop images
* warp images
* resample images
* otherwise modify images to make them compatible

Dimension mismatches must produce a clear error.

---

# 10. Output

Default output is machine-readable JSON.

Example:

```json
{
  "selection": {
    "type": "uv-rect",
    "pixel_count": 16000
  },
  "evaluation": {
    "type": "average-color"
  },
  "result": {
    "r": 0.42,
    "g": 0.51,
    "b": 0.37,
    "a": 1.0
  }
}
```

Output should include enough metadata to identify:

* selector
* selector parameters
* evaluator
* evaluator parameters
* image dimensions
* selected pixel count
* numerical result

Errors must also be machine-readable.

Example:

```json
{
  "error": {
    "code": "INVALID_COORDINATE",
    "message": "UV coordinate must be within [0, 1].",
    "parameter": "u"
  }
}
```

Invalid queries and processing failures must return a non-zero exit status.

---

# 11. Determinism

All operations must be deterministic.

The same:

* image files
* selector
* selector parameters
* evaluator
* evaluator parameters

must produce the same result.

Ordering of returned collections must be deterministic.

Floating-point serialization must use a consistent precision.

---

# 12. Validation

Validate:

* selector name
* evaluator name
* selector parameters
* evaluator parameters
* parameter types
* coordinate ranges
* tolerance ranges
* image existence
* supported image formats
* image count
* image dimensions
* predicate syntax
* required non-empty inputs

Invalid input must fail explicitly.

Do not silently clamp invalid input.

---

# 13. Tests

Tests validate the render-debug tool itself, not a graphics application.

## 13.1 Unit tests

Cover:

* image decoding
* coordinate conversion
* UV-to-pixel mapping
* rectangle inclusion
* radius inclusion
* color matching
* predicate parsing
* predicate evaluation
* channel extraction
* every evaluator
* empty selections
* invalid parameters
* deterministic ordering
* floating-point serialization

## 13.2 Selector tests

Use small deterministic fixture images with known pixel values.

Every selector must have:

* normal-case tests
* boundary tests
* invalid-input tests

## 13.3 Evaluator tests

Every evaluator must have deterministic fixtures with known expected results.

Include:

* uniform images
* single-channel differences
* multi-channel differences
* alpha values where applicable
* known image pairs for comparison evaluators
* empty selections where applicable

## 13.4 CLI integration tests

Test complete invocations such as:

```text
render-debug --select pixel 10 20 --evaluate pixel-value --images image.png
```

```text
render-debug --select uv-rect 0.3 0.4 0.5 0.8 --evaluate average-color --images image.png
```

```text
render-debug --select uv-rect 0.3 0.4 0.5 0.8 --evaluate error --images reference.png actual.png
```

Verify:

* exit code
* JSON structure
* numerical results
* validation behavior

---

# 14. V1 Selector Registry

The implementation must expose exactly these selector types:

| Selector     | Function                                   |
| ------------ | ------------------------------------------ |
| `all`        | Entire image                               |
| `pixel`      | Exact pixel by XY                          |
| `uv`         | Exact location by normalized UV            |
| `uv-rect`    | Rectangular normalized-UV region           |
| `pixel-rect` | Rectangular pixel region                   |
| `uv-radius`  | Circular normalized-UV region              |
| `color`      | Pixels matching or near a color            |
| `predicate`  | Pixels satisfying channel/value conditions |

---

# 15. V1 Evaluator Registry

The implementation must expose exactly these evaluator types:

| Evaluator            | Function                                      |
| -------------------- | --------------------------------------------- |
| `pixel-value`        | Exact selected pixel value                    |
| `average-color`      | Mean channel values                           |
| `min-max`            | Minimum and maximum channel values            |
| `percentiles`        | Requested channel percentiles                 |
| `statistics`         | Descriptive channel statistics                |
| `dominant-colors`    | Most prevalent distinct colors                |
| `color-distribution` | Sampled channel/color distributions           |
| `nearest-color`      | Pixel nearest to a target color               |
| `color-count`        | Count/percentage matching a color             |
| `color-range`        | Per-channel color range                       |
| `histogram`          | Fixed-bin channel distributions               |
| `occupancy`          | Fraction satisfying a condition               |
| `count`              | Number of selected pixels                     |
| `error`              | Aggregate image-to-image error                |
| `error-distribution` | Distribution of image-to-image error          |
| `channel-error`      | Per-channel image-to-image error              |
| `color-difference`   | Difference between selected color populations |
| `match`              | Image-to-image equivalence under tolerance    |

---

# 16. V1 Exclusions

Do not implement:

* renderer/graphics-pipeline inspection
* scene inspection
* geometry extraction
* object recognition
* contour extraction
* segmentation
* automatic diagnosis
* automatic correction
* image capture
* renderer integration
* visualization commands
* `bounds`
* `centroid`
* selectors not listed in the V1 selector registry
* evaluators not listed in the V1 evaluator registry
