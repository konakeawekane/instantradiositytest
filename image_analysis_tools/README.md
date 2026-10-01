# Render Debug Tools v1

`render-debug` is a deterministic, JSON-first image interrogation CLI for coding agents. It analyzes image files without inspecting renderer, scene, geometry, shader, or graphics-pipeline state.

For autonomous use, follow [`AGENT_PROTOCOL.md`](AGENT_PROTOCOL.md). Its machine-readable companion, `agent_protocol.json`, defines activation rules, staged query budgets, recipes, and stop conditions.

## Install and run

```bash
python -m pip install -e .
render-debug --select all --evaluate statistics --images image.png
```

Without installation:

```bash
python -m render_debug --select all --evaluate statistics --images image.png
```

Successful results are JSON on stdout. Validation and processing failures are JSON on stderr and use a nonzero exit status. Add `--compact` for one-line JSON. JSON serialization uses Python's deterministic shortest round-trip float representation and rejects NaN/Infinity.

## Query model

Every query has one selector, one evaluator, and the evaluator's required number of images:

```text
render-debug --select <selector> [values...] \
  --evaluate <evaluator> [evaluator flags] \
  --images <path> [<path> ...]
```

The CLI normalizes hyphenated public names into independent selection and evaluation objects before loading images. Exactly eight selectors and eighteen evaluators are exposed.

## Coordinates and channels

- Pixel coordinates are zero-based with `(0, 0)` at the top-left.
- UV coordinates are finite values in `[0, 1]`, also from top-left to bottom-right.
- A UV point maps with `min(floor(uv * dimension), dimension - 1)`, so UV `1` maps to the final pixel.
- Pixel centers are `((x + 0.5) / width, (y + 0.5) / height)`.
- `uv-rect` and `uv-radius` inclusion tests use those pixel centers.
- Integer channels are normalized to `[0, 1]` for analysis. Floating-point/unbounded channels retain native values.
- Alpha is retained and reported. RGB target colors intentionally ignore alpha; RGBA targets include it.
- No gamma linearization or implicit color-space conversion is applied to supported native modes. Palette/1-bit conversions are reported as warnings.

## Selectors

| Selector | Values | Behavior |
|---|---|---|
| `all` | none | Every pixel, row-major |
| `pixel` | `x y` | One in-bounds integer coordinate |
| `uv` | `u v` | One pixel using the mapping above |
| `uv-rect` | `u_min v_min u_max v_max` | Pixel centers inside inclusive UV edges; equal min/max yields empty |
| `pixel-rect` | `x_min y_min x_max y_max` | Inclusive integer bounds |
| `uv-radius` | `u v radius` | Pixel centers whose squared UV distance is at most `radius²` |
| `color` | `hex [tolerance]` | Pixels within color tolerance; default tolerance `0` |
| `predicate` | quoted expression | Pixels satisfying an RGBA condition |

Examples:

```bash
render-debug --select pixel-rect 10 20 30 40 --evaluate count --images image.png
render-debug --select uv-radius 0.5 0.5 0.1 --evaluate average-color --images image.png
render-debug --select color "#FF0000" 0.02 --evaluate count --images image.png
render-debug --select predicate "R > 0.8 AND (B < 0.2 OR A < 1)" --evaluate count --images image.png
```

Predicates support `R`, `G`, `B`, `A`; `>`, `>=`, `<`, `<=`, `==`, `!=`; case-insensitive `AND`/`OR`; and parentheses. `AND` binds more tightly than `OR`. Missing alpha is represented as `1`.

### Color distance

The `color` selector, `color-count`, and `nearest-color` share root-mean-square distance on normalized channels:

```text
distance = sqrt(sum((pixel_channel - target_channel)^2) / compared_channel_count)
```

Six-digit RGB targets compare three channels and ignore alpha. Eight-digit RGBA targets compare four channels.

## Single-image evaluators

| Evaluator | Required flags | Result |
|---|---|---|
| `pixel-value` | none | Exact native value, normalized channels, coordinate, UV center, and hex where meaningful; selection must contain one pixel |
| `average-color` | none | Arithmetic mean per available normalized/native channel |
| `min-max` | none | Minimum and maximum per channel |
| `percentiles` | `--percentiles 1,25,50,75,99` | Requested percentile values per channel |
| `statistics` | none | Count, mean, median, population variance, standard deviation, min, max |
| `dominant-colors` | `--top N` | Deterministically clustered colors ordered by count then cluster key |
| `color-distribution` | `--samples N` | Exactly N evenly spaced percentile samples per channel |
| `nearest-color` | `--color "#RRGGBB[AA]"` | Nearest selected pixel, UV, color, and distance |
| `color-count` | `--color ... --tolerance T` | Matching count and percentage |
| `color-range` | none | Minimum, maximum, and range per channel |
| `histogram` | `--bins N` | Fixed bins over `[0, 1]` with underflow/overflow counts |
| `occupancy` | `--condition "..."` | Count, total, fraction, and percentage satisfying a predicate |
| `count` | none | Selected pixel count |

Percentiles use Hyndman–Fan type 7 interpolation:

```text
rank = (N - 1) * percentile / 100
value = linear interpolation between floor(rank) and ceil(rank)
```

`color-distribution` uses percentiles `0..100` inclusive when `N > 1`; one sample is the 50th percentile.

`dominant-colors` groups each normalized RGB/RGBA channel into a fixed 5-bit bin using `min(31, floor(channel * 32))`. The displayed color is the arithmetic mean of the original values in the cluster. Results sort by descending count, then lexicographic cluster key.

Histogram bins are `[lower, upper)`; the final bin includes `1.0`. Float values below `0` or above `1` are reported as underflow/overflow instead of silently clamped.

## Two-image evaluators

| Evaluator | Required flags | Result |
|---|---|---|
| `error` | none | Total, mean, maximum, and per-channel normalized absolute error |
| `error-distribution` | `--bins N` | Histogram of per-pixel mean absolute error |
| `channel-error` | none | Total, mean, and maximum absolute error independently per channel |
| `color-difference` | none | Signed channel-mean differences and channel-distribution differences |
| `match` | `--tolerance T` | Whether maximum corresponding channel error is within tolerance |

Two-image operations require identical dimensions, working modes, channels, and corresponding selected coordinates. Images are never resized, cropped, warped, or resampled. Content-dependent selectors (`color` and `predicate`) are applied independently to both images and rejected if their selected coordinates differ.

For normalized channel values `r(p,c)` and `a(p,c)`:

```text
channel error(p,c) = abs(r(p,c) - a(p,c))
error total = sum over pixels and channels of channel error
error mean = error total / (pixel count * channel count)
error maximum = maximum channel error
per-pixel error = mean channel error at that pixel
```

`error-distribution` bins the same per-pixel error metric. `match` compares the maximum channel error to its tolerance.

`color-difference` reports:

- signed mean difference: `mean(actual) - mean(reference)` per channel;
- distribution difference: one-dimensional Wasserstein distance, implemented as the mean absolute difference between the sorted, equal-size channel populations.

## Empty selections

Zero selected pixels are valid. Behavior is explicit:

- `count`, `color-count`, `histogram`, `occupancy`, `error`, `error-distribution`, `channel-error`, and `match` return zero-valued results (`match` is true).
- Evaluators requiring a defined sample statistic or a nearest pixel return `empty_selection`.
- `pixel-value` returns `invalid_selection_size` unless the selection contains exactly one pixel.

## Image handling

Native analysis is supported for Pillow modes `L`, `LA`, `RGB`, `RGBA`, `CMYK`, `I`, `F`, and 16-bit integer modes. Palette images convert to RGB/RGBA and 1-bit images to `L`, with a warning in the JSON result. Results include dimensions, source/working modes, channels, numeric type, known channel maximum, gamma when present, and ICC-profile presence.

## Development and verification

```bash
python -m compileall -q render_debug tests
python -m unittest discover -s tests -t . -v
```

Tests use tiny deterministic images with hand-checkable values and cover decoding, all selectors, predicate parsing/evaluation, all evaluators, alpha, floating-point channels, empty selections, invalid parameters, deterministic ordering, JSON behavior, and complete CLI invocations.
