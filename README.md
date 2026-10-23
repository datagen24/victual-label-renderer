# victual-label-renderer

The headless renderer [plan 27](https://github.com/datagen24/victual/blob/master/docs/plans/27-label-templates-and-rendering.md)
piece 4 specifies. It reads Victual's template document, its media profile and its captured
fields, and produces one validated `raster/png-indexed;v=1` artifact.

**It touches no printer.** That is not a convention here, it is the reason the split exists:
a render may be retried automatically precisely because it cannot produce a physical label,
while a delivery attempt is never retried automatically at all
([ADR-0019](https://github.com/datagen24/victual/blob/master/docs/adr/0019-label-printers-are-master-data.md)
decision item 6). The renderer is given no printer address and no socket, so an automatically
retried render cannot print by construction rather than by discipline. Delivery lives in
[victual-label-worker](https://github.com/datagen24/victual-label-worker).

## Why this runtime

Selected 2026-09-07 by [ADR-0021](https://github.com/datagen24/victual/blob/master/docs/adr/0021-label-templates-are-application-data.md)
prerequisite 1, by rendering the template contract rather than by reading feature lists.
Three candidates were compared over multi-line wrapping, a pinned font with a missing glyph,
QR at a declared module size with asymmetric resolutions, black/red output and continuous
length. A single Rust binary over `usvg`/`tiny-skia` was the only one that emitted a
palette-conformant artifact and the only one with neither shell nor interpreter in its
closure — 62,632,768 bytes over seven paths, against 316 MB and one interpreter reference for
the Pillow environment and 1.8 GB and two for Chromium.

Two properties decided it, and both are geometry rather than taste:

- **One engine measures and paints.** The emitter-plus-CLI candidate measured with `hmtx`
  advances and painted with rustybuzz; those disagreed by up to 5.77% on kerning-heavy
  strings. Here the text node that is measured is the text node that is painted.
- **Anisotropy is a transform on outlines, not a resample.** A point is 1/72 inch, so 11 pt
  on a 300 × 600 device is a nominal em of 45.8 × 91.7 device pixels. Pillow exposes no
  FreeType transform and can only get there by resampling the text layer, which plan 27
  piece 3 forbids.

## What it does not decide

**It does not encode the QR.** Victual computes the module matrix and hands it over. That
removes a second QR implementation that could disagree with the first, keeps a symbology
library out of this closure, and — the reason it is done this way — lets Victual verify a
finished artifact by re-encoding the pinned payload and comparing modules. Both sides
choosing a mask independently would make that comparison meaningless.

**It does not decide what a label says.** Captured values arrive with the request, already
authorized and read by Victual in the transaction that created them.

## Running it

```
victual-label-renderer --api https://victual.example --key-file /run/secrets/renderer-key
```

One invocation claims one durable render request, produces one artifact, submits it and
exits — so the deployment scales to zero and the dispatcher finds queued work whether or not
anything is running. `--health` exits 0 without contacting anything, which is the probe
[ADR-0010](https://github.com/datagen24/victual/blob/master/docs/adr/0010-workload-standard.md)
rule 4 requires from an image with no shell.

## Artifact form

`raster/png-indexed;v=1`: PNG colour type 3 at bit depth 2 over the profile's palette, with
the pixel grid fixed by the resolved combination. The form identifier pins colour type and
bit depth rather than merely "PNG", because without that the same name spans artifacts
differing by an order of magnitude in size and by whether the worker has to quantise before
it can send. `tiny_skia`'s own `save_png` writes RGBA8 — 37,145 bytes where the indexed
encoding of identical pixels is 2,573 — so the encoder here is the renderer's own.

## Text that does not fit

A text element's box bounds it across and down. Lines start at the top of the box, one line
advance (`size_pt × line_spacing`) apart, with the first baseline one advance down, so `n`
lines need `n` advances of height. When the value does not fit, `overflow` decides:

- `error` refuses the render with `TEXT_OVERFLOW`, naming the element and saying whether
  the value was too wide or needed too many lines.
- `ellipsis` keeps the lines that fit and ends the last one with `…`. A value too wide for
  the box becomes one ellipsised line, as before. A box too short for one line refuses.
- `shrink_to_fit` steps the size down by 0.5 pt until the value fits both ways, and refuses
  at `min_size_pt`.

Until 2026-10-23 only a turned box was bounded down, and only under `error`. An upright box
drew every line it needed over whatever sat below it.
