# KeptCraft project format

A `.keptcraft` file is one self-contained JSON object holding a screenshot and every edit made to
it, so reopening it needs no side files.
It is written as a single line and replaced atomically, so a watcher never reads a half-written
project.
The extension the fork had before the rename, `.shotcraft`, still opens.
When an image has a same-name `.keptcraft` sidecar, opening the image opens that project. Saving a
project updates only the project file; the source image stays unchanged, and **Save As** exports a
flattened image.

The definition lives in [`model.rs`](../crates/doc/src/model.rs).
Its test `the_format_doc_names_every_field_and_tag` reads the serialized model and fails when this
page and the code drift apart.

## Envelope

| Field | Type | Meaning |
|---|---|---|
| `format` | string | Always `keptcraft-project`. Any other value is refused as "not a project". |
| `version` | number | Format version, currently `1`. A newer version is refused rather than guessed at. |
| `base` | string | The original capture as a base64 PNG (RGBA8). |
| `items` | array | Annotations, painted in array order, so the last one sits on top. |
| `crop` | object or null | The visible part of the base image, in base pixels. |
| `background` | object or null | The canvas behind the image: padding, fill, shadow. |
| `commentary` | object or null | The comment band beside the image. |

`crop`, `background` and `commentary` may be left out; a value that cannot be read fails the load
with an error rather than silently opening an empty document.
Unknown top-level keys are ignored, so a tool may add its own without breaking KeptCraft.

## Geometry

All coordinates and lengths are in **base-image pixels**, with the origin at the top-left corner of
the uncropped capture.
A rectangle is an object with a `min` and a `max` corner, each named by `x` and `y`, for example
`{"min":{"x":27.0,"y":52.5},"max":{"x":112.0,"y":165.5}}`.
A point or an offset is the same `x` and `y` pair on its own.
Numbers are plain JSON doubles.

Nothing is destructive: `crop` only narrows what is shown, and a `background` is drawn around the
image at render time.
Redaction is an annotation too, so the pixels under it stay in `base` (see [Security](#security)).

## Items

| Field | Type | Meaning |
|---|---|---|
| `shape` | object | The annotation itself: exactly one tag names its kind and its fields. |
| `color` | array | `[r,g,b,a]`, 0-255 each. |
| `width` | number | Stroke width in pixels; it also scales arrow heads, counters and pixelation. |

## Shapes

The tag is the single key under `shape`, in the style of serde's externally tagged enums:
`{"shape":{"Rect":{"rect":{…},"filled":true}}}`.

| Tag | Fields |
|---|---|
| `Rect` | `rect`, `filled` (bool) |
| `Ellipse` | `rect`, `filled` (bool) |
| `Line` | `a`, `b` |
| `Arrow` | `a`, `b`, `style`, `bend` (curve control in pixels) |
| `Pen` | `pts`, the points of the stroke |
| `Highlighter` | `pts`, drawn wide and translucent |
| `Text` | `pos`, `text`, `size`, `style`, `wrap` (paragraph width, or null for one line) |
| `Counter` | `center`, `n` (the step number), `style`, `label` (a name of the user's own, or null) |
| `Redact` | `rect`, `mode`, `seed` (keeps pixelation stable across redraws) |
| `Spotlight` | `rect`, `ellipse` (bool) |
| `Image` | `rect`, `image` (a base64 PNG pasted into the document) |

### Arrow styles

`style` is one of `Tapered`, `Classic`, `Double` or `Curved`.

### Text styles

`style` is one of `Plain`, `Outlined`, `Shadow`, `Filled`, `Light`, `Dark` or `Stroke`.
The last three draw a box behind the text.

### Redaction modes

`mode` is one of `Pixelate`, `SecureBlur`, `Blur` or `Solid`.

### Counter numbers

`label` is `Numbers` or `Letters` (`1`, `2`, `3` or `A`, `B`, `C`, then `AA`).
A counter's own `label` string, when set, replaces the number on the image.

### Counter look

The `style` object of a `Counter`:

| Field | Meaning |
|---|---|
| `size` | Circle diameter in base pixels. |
| `label` | `Numbers` or `Letters`. |
| `outline` | Ring colour as `[r,g,b,a]`, or null for the fill only. |
| `outline_width` | Ring width in pixels. |
| `shadow` | Drop-shadow strength, 0 to 1. |
| `shadow_on` | Whether the shadow is drawn at all. |
| `shadow_blur` | Reach of the shadow's soft edge, in pixels. |
| `shadow_angle` | Direction the shadow falls, in degrees: 0 right, 90 down, 180 left, 270 up. |
| `shadow_distance` | Distance from the circle, in pixels. |
| `padding` | Fraction of the radius kept clear between the label and the edge. |

## Background

| Field | Meaning |
|---|---|
| `fill` | What is behind and around the screenshot; see below. |
| `padding` | Space around the screenshot, in base pixels. |
| `radius` | Corner radius of the screenshot, in base pixels. |
| `shadow` | Shadow strength, 0 to 1. |
| `aspect` | `Auto`, or `Ratio` with `[width,height]` for a pinned aspect. |
| `align` | Where the image sits in the canvas: a pair of `Min`, `Center` or `Max`, horizontal then vertical. |
| `auto_balance` | Trim uneven uniform margins so the content sits evenly in its padding. |
| `watermark` | A mark drawn over the finished image, or null. |

### Fills

| Tag | Payload |
|---|---|
| `Transparent` | Nothing. |
| `Gradient` | The index of one of the built-in gradients. |
| `Custom` | A gradient the user built: `angle` in degrees (0 left to right, 90 top to bottom) and `stops`, each with `at` (0 to 1) and `color` as `[r,g,b,a]`. |
| `Solid` | One colour, `[r,g,b,a]`. |
| `Image` | A base64 PNG scaled to the canvas. |
| `Blurred` | The screenshot itself, scaled to cover and heavily blurred. |

### Watermark

| Field | Meaning |
|---|---|
| `image` | The mark as a base64 PNG. |
| `anchor` | Which edge it is placed against. |
| `width` | Its width as a fraction of the canvas width, so it keeps its proportion. |
| `offset` | Where the user dragged it from that spot, in canvas pixels. |
| `opacity` | 0 invisible to 1 opaque. |

`anchor` is one of `TopLeft`, `TopCenter`, `TopRight`, `MiddleLeft`, `MiddleCenter`,
`MiddleRight`, `BottomLeft`, `BottomCenter` or `BottomRight`.

## Commentary band

The band sits on one edge of the image and carries one line per numbered step.

| Field | Meaning |
|---|---|
| `background` | Band colour, `[r,g,b,a]`. |
| `text_color` | Comment colour, `[r,g,b,a]`. |
| `size` | Text size in base pixels. |
| `padding` | Space above the first line, below the last, and on both sides. |
| `entries` | The comments, one per step: `n` (the step number), `text`, and `spans` for bold and italic runs. |
| `side` | `Top`, `Right`, `Bottom` or `Left`. |
| `width` | Width of a side band in base pixels; 0 fits the comments to the image's height. |
| `match_counter` | Ring each step in the colour of its own counter instead of `badge`. |
| `badge` | Badge colour when matching is off or the step has no counter. |
| `header` | A title over the comments, or null. |
| `spacing` | Gap between the header and each step, in base pixels. |

Each entry of `spans` is `start` and `end` (character offsets into `text`, end exclusive), `bold`
and `italic`.

## Example

```jsonc
{
  "format": "keptcraft-project",
  "version": 1,
  "base": "<base64 of the capture PNG>",
  "items": [
    {
      "shape": { "Rect": { "rect": { "min": { "x": 27.0, "y": 52.5 }, "max": { "x": 112.0, "y": 165.5 } }, "filled": true } },
      "color": [255, 59, 48, 255],
      "width": 6.0
    }
  ],
  "crop": null,
  "background": { "fill": { "Solid": [175, 82, 222, 255] }, "padding": 64.0, "radius": 12.0, "shadow": 0.5, "aspect": "Auto", "align": ["Center", "Center"], "auto_balance": true, "watermark": null },
  "commentary": null
}
```

## Security

A project is executable input from another program, so it is decoded defensively: image dimensions
are capped and a file that fails to parse is reported, never replaced.

Redaction and cropping are edits, not erasures.
The `base` image keeps the original capture, and so does the project KeptCraft stores beside every
history entry, so a project made from a redacted screenshot still contains what was hidden.
