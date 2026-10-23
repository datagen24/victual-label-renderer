//! Template document to indexed raster.
//!
//! Two rules govern every number in this file, and both come from plan 27 piece 1.
//!
//! **Geometry is physical.** The document carries millimetres and points; device pixels are
//! computed here, per axis, by the profile's stated rounding. Horizontal geometry resolves
//! against `dpi_x` and vertical against `dpi_y` - a point is 1/72 inch, so 11 pt on a
//! 300 x 600 device is a nominal em of 45.8 x 91.7 device pixels. Two candidates in the
//! 2026-09-07 comparison sized at `size_pt * dpi_y / 72` and then measured horizontal
//! advances in that space, making every string twice as wide as its physical size.
//!
//! **The engine that measures is the engine that paints.** Line breaking asks `usvg` for the
//! advance of the very text node that will be rendered. Measuring with `hmtx` advances and
//! painting with a shaper is what diverged by up to 5.77% on kerning-heavy strings, and
//! summing advances is not what shaping produces.
//!
//! **Orientation turns pixels, never outlines at the wrong scale.** A schema 2 document may
//! set `canvas.orientation_deg` to 0, 90, 180 or 270. Its elements are positioned in the
//! reading frame; canvas `width_mm` stays the roll's width and `height_mm` the length along
//! the tape. The reading frame is rendered with each axis at the dpi of the physical axis it
//! will become, then turned clockwise onto the raster by `turn_pixmap`, an exact permutation. A schema 2 element may
//! carry `rotation_deg` too: it is painted in its own frame the same way, turned by the same
//! permutation, and placed with its footprint's top-left corner at (`x_mm`, `y_mm`).

use serde_json::Value;

use crate::png;

const MM_PER_INCH: f32 = 25.4;
const POINTS_PER_INCH: f32 = 72.0;

pub struct Fail {
    pub code: &'static str,
    pub element: Option<String>,
    pub detail: String,
}

pub fn fail(code: &'static str, element: Option<&str>, detail: impl Into<String>) -> Fail {
    Fail { code, element: element.map(str::to_string), detail: detail.into() }
}

pub struct Rendered {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

struct Geometry {
    dpi_x: f32,
    dpi_y: f32,
    width: u32,
}

impl Geometry {
    /// The profile's `pixel_policy.rounding` is "nearest", stated once there and applied
    /// everywhere. Victual computes the QR's module origin the same way, which is what lets
    /// it verify a finished artifact by sampling module centres.
    fn x(&self, mm: f32) -> f32 {
        (mm * self.dpi_x / MM_PER_INCH).round()
    }
    fn y(&self, mm: f32) -> f32 {
        (mm * self.dpi_y / MM_PER_INCH).round()
    }
}

pub fn render(input: &Value, assets: &[(String, Vec<u8>)], _version: &str) -> Result<Rendered, Fail> {
    let document = input.get("document").ok_or_else(|| fail("UNSUPPORTED_VERSION", None, "no document"))?;
    let profile = input.get("profile").ok_or_else(|| fail("MEDIA_INCOMPATIBLE", None, "no profile"))?;

    let schema = document.get("schema_version").and_then(Value::as_i64);
    if schema != Some(1) && schema != Some(2) {
        return Err(fail("UNSUPPORTED_VERSION", None, "only template schema_version 1 and 2 are understood"));
    }
    let orientation = match (schema, document["canvas"].get("orientation_deg")) {
        (_, None) => 0,
        (Some(2), Some(value)) => match value.as_i64() {
            Some(degrees @ (0 | 90 | 180 | 270)) => degrees as u32,
            _ => return Err(fail("UNSUPPORTED_VERSION", None, "orientation_deg is one of 0, 90, 180 and 270")),
        },
        _ => return Err(fail("UNSUPPORTED_VERSION", None, "orientation_deg needs template schema_version 2")),
    };

    let physical = Geometry {
        dpi_x: number(profile, "dpi_x"),
        dpi_y: number(profile, "dpi_y"),
        width: number(profile, "raster_width_px") as u32,
    };
    if physical.width == 0 || physical.dpi_x <= 0.0 || physical.dpi_y <= 0.0 {
        return Err(fail("MEDIA_INCOMPATIBLE", None, "the profile fixes no usable pixel grid"));
    }

    // Everything below is laid out and painted in the reading frame, the design as somebody
    // reads it. On a quarter turn, reading x lands on the tape's length and reading y on the
    // roll's width, so each reading axis takes the dpi of the physical axis it becomes: the
    // outlines are scaled for the device before the pixels are turned, never after.
    let quarter = orientation == 90 || orientation == 270;
    let mut geometry = if quarter {
        Geometry { dpi_x: physical.dpi_y, dpi_y: physical.dpi_x, width: 0 }
    } else {
        Geometry { dpi_x: physical.dpi_x, dpi_y: physical.dpi_y, width: physical.width }
    };

    let palette: Vec<[u8; 3]> = profile["pixel_policy"]["palette"]
        .as_array()
        .ok_or_else(|| fail("MEDIA_INCOMPATIBLE", None, "the profile carries no palette"))?
        .iter()
        .map(|v| png::parse_colour(v.as_str().unwrap_or("")).map_err(|e| fail("MEDIA_INCOMPATIBLE", None, e)))
        .collect::<Result<_, _>>()?;
    let permits_red = palette.iter().any(|c| *c == [0xff, 0x00, 0x00]);

    let elements = document["elements"].as_array().ok_or_else(|| fail("UNSUPPORTED_VERSION", None, "no elements"))?;
    for element in elements {
        if let Some(value) = element.get("rotation_deg") {
            let id = element["id"].as_str();
            if schema != Some(2) {
                return Err(fail("UNSUPPORTED_VERSION", id, "rotation_deg needs template schema_version 2"));
            }
            if !matches!(value.as_i64(), Some(0 | 90 | 180 | 270)) || element["type"] == "line" {
                return Err(fail("UNSUPPORTED_VERSION", id, "rotation_deg is one of 0, 90, 180 and 270, on a text, qr, image or rect element"));
            }
        }
    }
    let fields = &input["capture"]["fields"];

    let mut fontdb = fontdb::Database::new();
    let mut families: Vec<(String, String)> = Vec::new();
    for (name, bytes) in assets {
        let before = fontdb.len();
        fontdb.load_font_data(bytes.clone());
        if fontdb.len() > before {
            if let Some(face) = fontdb.faces().last() {
                if let Some((family, _)) = face.families.first() {
                    families.push((name.clone(), family.clone()));
                }
            }
        }
    }
    let family_of = |asset: &str| -> Option<String> {
        families.iter().find(|(n, _)| n == asset).map(|(_, f)| f.clone())
    };

    // Lay out the text first: automatic height is content extent plus explicit spacing, so
    // the canvas cannot be sized until the lines are known.
    let mut laid_out: Vec<(usize, Vec<String>, f32, f32)> = Vec::new();
    for (index, element) in elements.iter().enumerate() {
        if element["type"] != "text" {
            continue;
        }
        let id = element["id"].as_str().unwrap_or("text");
        let asset = element["font_asset"].as_str().unwrap_or_default();
        let family = family_of(asset)
            .ok_or_else(|| fail("ASSET_UNAVAILABLE", Some(id), format!("no font asset named {asset}")))?;

        let text = match element["field"].as_str() {
            Some(field) => fields
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| fail("ASSET_UNAVAILABLE", Some(id), format!("the capture carries no {field}")))?
                .to_string(),
            None => element["literal"].as_str().unwrap_or_default().to_string(),
        };

        // A missing glyph is an error rather than silent substitution, and neither rasterizer
        // reports one - so the font's own cmap is asked before anything is drawn.
        if let Some((_, bytes)) = assets.iter().find(|(n, _)| n == asset) {
            if let Ok(face) = ttf_parser::Face::parse(bytes, 0) {
                let missing: String = text
                    .chars()
                    .filter(|c| !c.is_whitespace() && face.glyph_index(*c).is_none())
                    .collect();
                if !missing.is_empty() {
                    return Err(fail("MISSING_GLYPH", Some(id), format!("the pinned font has no glyph for {missing:?}")));
                }
            }
        }

        // A turned text box runs along whichever axis it was turned onto, so it is measured
        // at that axis's dpi: the same frame swap as a turned canvas, one element at a time.
        let geometry = &local(&geometry, rotation_of(element));
        let mut size_pt = number(element, "size_pt");
        let minimum = element["min_size_pt"].as_f64().map(|v| v as f32);
        let overflow = element["overflow"].as_str().unwrap_or("error");
        let wrap = element["wrap"].as_bool().unwrap_or(true);
        let box_width = geometry.x(number(element, "width_mm"));
        // The box is bounded down as well as across. Lines are laid out from the top of the
        // box one line advance apart, the first baseline one advance down (text_svg), so n
        // lines need n advances; half a pixel of slack absorbs the millimetre rounding.
        let box_height = geometry.y(number(element, "height_mm")) + 0.5;
        let spacing = number(element, "line_spacing");

        let (lines, used) = loop {
            let font_px = size_pt * geometry.dpi_x / POINTS_PER_INCH;
            let line_height = size_pt * geometry.dpi_y / POINTS_PER_INCH * spacing;
            let lines = break_lines(&text, &family, font_px, &fontdb, box_width, wrap);
            let fits_across = lines.iter().all(|line| measure(line, &family, font_px, &fontdb) <= box_width);
            let fits_down = line_height * lines.len() as f32 <= box_height;
            if fits_across && fits_down {
                break (lines, size_pt);
            }
            let room = (box_height / line_height).floor() as usize;
            match (overflow, minimum) {
                ("shrink_to_fit", Some(floor)) if size_pt - 0.5 >= floor => size_pt -= 0.5,
                ("ellipsis", _) if room >= 1 => {
                    // Too wide keeps the single ellipsised line it always produced. Too many
                    // lines keeps the ones that fit, and the last of them ends the text.
                    if !fits_across {
                        break (ellipsise(&text, &family, font_px, &fontdb, box_width), size_pt);
                    }
                    let mut kept: Vec<String> = lines[..room - 1].to_vec();
                    kept.push(ellipsise(&lines[room - 1..].join(" "), &family, font_px, &fontdb, box_width).remove(0));
                    break (kept, size_pt);
                }
                _ if !fits_across => {
                    return Err(fail("TEXT_OVERFLOW", Some(id),
                        format!("{text:?} does not fit a {:.1} mm wide box at {size_pt} pt", number(element, "width_mm"))))
                }
                _ => {
                    return Err(fail("TEXT_OVERFLOW", Some(id),
                        format!("{text:?} needs {} line(s), {:.1} mm, at {size_pt} pt and the box is {:.1} mm tall",
                            lines.len(), line_height * lines.len() as f32 * MM_PER_INCH / geometry.dpi_y, number(element, "height_mm"))))
                }
            }
        };
        laid_out.push((index, lines, used, spacing));
    }

    let height = if quarter {
        geometry.width = canvas_length_across(document, profile, &geometry, &physical, elements, input)?;
        physical.width
    } else {
        canvas_height(document, profile, &geometry, elements, &laid_out, input)?
    };

    // Non-text elements are painted directly, so a QR module lands on exactly the pixels
    // Victual will sample. Text goes through resvg afterwards and composites over them.
    let mut pixmap = tiny_skia::Pixmap::new(geometry.width, height)
        .ok_or_else(|| fail("RENDER_FAILED", None, "pixmap allocation failed"))?;
    pixmap.fill(tiny_skia::Color::WHITE);

    for element in elements {
        let id = element["id"].as_str().unwrap_or("element");
        let colour = colour_of(element, "color", permits_red).map_err(|e| fail("MEDIA_INCOMPATIBLE", Some(id), e))?;
        if rotation_of(element) != 0 || element["type"] == "text" {
            continue;
        }
        match element["type"].as_str().unwrap_or_default() {
            "qr" => paint_qr(&mut pixmap, &geometry, element, input, colour)?,
            "rect" => paint_rect(&mut pixmap, &geometry, element, colour, permits_red)?,
            "line" => paint_line(&mut pixmap, &geometry, element, colour),
            "image" => paint_image(&mut pixmap, &geometry, element, assets, colour)?,
            _ => {}
        }
    }

    let fontdb = std::sync::Arc::new(fontdb);
    let upright: Vec<(usize, Vec<String>, f32, f32)> = laid_out.iter()
        .filter(|(index, ..)| rotation_of(&elements[*index]) == 0)
        .cloned().collect();
    if !upright.is_empty() {
        let svg = text_svg(&geometry, height, elements, &upright, &families, fields, permits_red)?;
        draw_svg(&mut pixmap, &svg, &fontdb)?;
    }

    // Turned elements, in document order: each is painted at the origin of its own frame,
    // turned by the same exact permutation as the canvas, and composited with its footprint's
    // top-left corner at (x_mm, y_mm).
    for (index, element) in elements.iter().enumerate() {
        let turn = rotation_of(element);
        if turn == 0 {
            continue;
        }
        let id = element["id"].as_str().unwrap_or("element");
        let colour = colour_of(element, "color", permits_red).map_err(|e| fail("MEDIA_INCOMPATIBLE", Some(id), e))?;
        let mut frame = local(&geometry, turn);
        let mut at_origin = element.clone();
        at_origin["x_mm"] = Value::from(0.0);
        at_origin["y_mm"] = Value::from(0.0);
        let (w, h) = local_size(&frame, element, input);
        frame.width = w;
        let mut piece = tiny_skia::Pixmap::new(w.max(1), h.max(1))
            .ok_or_else(|| fail("RENDER_FAILED", Some(id), "pixmap allocation failed"))?;
        match element["type"].as_str().unwrap_or_default() {
            "qr" => paint_qr(&mut piece, &frame, &at_origin, input, colour)?,
            "rect" => paint_rect(&mut piece, &frame, &at_origin, colour, permits_red)?,
            "image" => paint_image(&mut piece, &frame, &at_origin, assets, colour)?,
            "text" => {
                // Bounded down at layout, in the element's own frame, like upright text.
                let (_, lines, size_pt, spacing) = laid_out.iter().find(|(i, ..)| *i == index)
                    .ok_or_else(|| fail("RENDER_FAILED", Some(id), "the text was not laid out"))?;
                let one = [at_origin];
                let svg = text_svg(&frame, h, &one, &[(0, lines.clone(), *size_pt, *spacing)], &families, fields, permits_red)?;
                draw_svg(&mut piece, &svg, &fontdb)?;
            }
            _ => {}
        }
        let piece = turn_pixmap(&piece, turn).ok_or_else(|| fail("RENDER_FAILED", Some(id), "pixmap allocation failed"))?;
        pixmap.draw_pixmap(
            geometry.x(number(element, "x_mm")) as i32, geometry.y(number(element, "y_mm")) as i32,
            piece.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    }

    let pixmap = turn_pixmap(&pixmap, orientation)
        .ok_or_else(|| fail("RENDER_FAILED", None, "pixmap allocation failed"))?;
    let indices = quantise(&pixmap, &palette, profile);
    let bytes = png::encode(pixmap.width(), pixmap.height(), &indices, &palette)
        .map_err(|e| fail("RENDER_FAILED", None, e))?;

    Ok(Rendered { bytes, width: pixmap.width(), height: pixmap.height() })
}

/// An element's clockwise turn; 0 when it states none.
fn rotation_of(element: &Value) -> u32 {
    element["rotation_deg"].as_u64().unwrap_or(0) as u32
}

/// The frame a turned element is laid out in: on a quarter turn its own x lands on the
/// surrounding y, so it takes that axis's dpi.
fn local(geometry: &Geometry, turn: u32) -> Geometry {
    if turn == 90 || turn == 270 {
        Geometry { dpi_x: geometry.dpi_y, dpi_y: geometry.dpi_x, width: geometry.width }
    } else {
        Geometry { dpi_x: geometry.dpi_x, dpi_y: geometry.dpi_y, width: geometry.width }
    }
}

/// An element's unturned size in pixels of its own frame.
fn local_size(frame: &Geometry, element: &Value, input: &Value) -> (u32, u32) {
    if element["type"] == "qr" {
        let id = element["id"].as_str().unwrap_or("qr");
        let size = input["qr"][id]["modules"].as_array().map_or(0, Vec::len) as u32;
        let quiet = element["quiet_zone_modules"].as_u64().unwrap_or(4) as u32;
        let module_mm = number(element, "module_mm");
        let span = size + 2 * quiet;
        return (span * frame.x(module_mm).max(1.0) as u32, span * frame.y(module_mm).max(1.0) as u32);
    }
    (frame.x(number(element, "width_mm")) as u32, frame.y(number(element, "height_mm")) as u32)
}

/// A turned element's footprint in the surrounding frame: width and height swap on a quarter turn.
fn footprint(geometry: &Geometry, element: &Value, input: &Value) -> (u32, u32) {
    let turn = rotation_of(element);
    let (w, h) = local_size(&local(geometry, turn), element, input);
    if turn == 90 || turn == 270 { (h, w) } else { (w, h) }
}

fn draw_svg(pixmap: &mut tiny_skia::Pixmap, svg: &str, fontdb: &std::sync::Arc<fontdb::Database>) -> Result<(), Fail> {
    let mut options = usvg::Options::default();
    options.fontdb = fontdb.clone();
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| fail("RENDER_FAILED", None, e.to_string()))?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Ok(())
}

/// Turns the reading-frame pixmap clockwise by `degrees` onto the physical raster.
///
/// The convention Victual's verifier and editor implement too: reading pixel (x, y) of a
/// W x H reading image lands at physical (H-1-y, x) for 90, (W-1-x, H-1-y) for 180 and
/// (y, W-1-x) for 270. An exact permutation of pixels, so nothing is resampled.
fn turn_pixmap(source: &tiny_skia::Pixmap, degrees: u32) -> Option<tiny_skia::Pixmap> {
    if degrees == 0 {
        return Some(source.clone());
    }
    let (w, h) = (source.width(), source.height());
    let (tw, th) = if degrees == 180 { (w, h) } else { (h, w) };
    let mut target = tiny_skia::Pixmap::new(tw, th)?;
    let from = source.pixels();
    let to = target.pixels_mut();
    for y in 0..h {
        for x in 0..w {
            let (px, py) = match degrees {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            to[(py * tw + px) as usize] = from[(y * w + x) as usize];
        }
    }
    Some(target)
}

/// Content extent plus explicit spacing, bounded by the profile's length rules.
///
/// Never crops and never grows without a bound: continuous tape is not an unbounded canvas,
/// so a document with automatic height states how far it may go and the media says whether
/// that is possible at all.
fn canvas_height(document: &Value, profile: &Value, geometry: &Geometry, elements: &[Value], laid_out: &[(usize, Vec<String>, f32, f32)], input: &Value) -> Result<u32, Fail> {
    let canvas = &document["canvas"];
    if let Some(fixed) = canvas["height_mm"].as_f64() {
        let height = geometry.y(fixed as f32) as u32;
        assert_length(profile, geometry, height)?;
        return Ok(height);
    }

    let mut bottom: f32 = 0.0;
    for (index, lines, size_pt, spacing) in laid_out {
        let element = &elements[*index];
        if rotation_of(element) != 0 {
            continue;
        }
        let top = geometry.y(number(element, "y_mm"));
        let line_height = size_pt * geometry.dpi_y / POINTS_PER_INCH * spacing;
        bottom = bottom.max(top + line_height * lines.len() as f32);
    }
    for element in elements {
        if rotation_of(element) != 0 {
            let (_, h) = footprint(geometry, element, input);
            bottom = bottom.max(geometry.y(number(element, "y_mm")) + h as f32);
            continue;
        }
        match element["type"].as_str().unwrap_or_default() {
            // The symbol and both quiet zones, in the same whole-pixel steps paint_qr uses,
            // so the canvas is never a pixel short of the symbol it is sized for. Leaving a
            // QR out made a QR-only label as long as its bottom margin (issue #948).
            "qr" => bottom = bottom.max(qr_bottom(geometry, element, input) as f32),
            "line" => bottom = bottom.max(geometry.y(number(element, "y1_mm")).max(geometry.y(number(element, "y2_mm")))),
            _ => bottom = bottom.max(geometry.y(number(element, "y_mm")) + geometry.y(number(element, "height_mm"))),
        }
    }
    let margin = number(&canvas["margins_mm"], "bottom");
    let height = (bottom + geometry.y(margin)).ceil().max(1.0) as u32;

    let cap = canvas["max_height_mm"].as_f64().unwrap_or(0.0) as f32;
    if cap > 0.0 && (height as f32) > geometry.y(cap) {
        return Err(fail("MEDIA_INCOMPATIBLE", None,
            format!("automatic height reached {:.1} mm against a max_height_mm of {cap}", height as f32 * MM_PER_INCH / geometry.dpi_y)));
    }
    assert_length(profile, geometry, height)?;
    Ok(height)
}

/// The reading width of a quarter-turned design: the tape length, in reading pixels.
///
/// The twin of canvas_height for the axis that runs along the tape once the design is
/// turned. Fixed `height_mm` is still the length along the tape, so it is measured on the
/// reading x axis here; automatic length is the content's right edge plus the right margin,
/// bounded the same way.
fn canvas_length_across(document: &Value, profile: &Value, geometry: &Geometry, physical: &Geometry, elements: &[Value], input: &Value) -> Result<u32, Fail> {
    let canvas = &document["canvas"];
    let length = if let Some(fixed) = canvas["height_mm"].as_f64() {
        geometry.x(fixed as f32) as u32
    } else {
        let mut right: f32 = 0.0;
        for element in elements {
            if rotation_of(element) != 0 {
                let (w, _) = footprint(geometry, element, input);
                right = right.max(geometry.x(number(element, "x_mm")) + w as f32);
                continue;
            }
            match element["type"].as_str().unwrap_or_default() {
                "qr" => right = right.max(qr_right(geometry, element, input) as f32),
                "line" => right = right.max(geometry.x(number(element, "x1_mm")).max(geometry.x(number(element, "x2_mm")))),
                _ => right = right.max(geometry.x(number(element, "x_mm")) + geometry.x(number(element, "width_mm"))),
            }
        }
        let margin = number(&canvas["margins_mm"], "right");
        let length = (right + geometry.x(margin)).ceil().max(1.0) as u32;
        let cap = canvas["max_height_mm"].as_f64().unwrap_or(0.0) as f32;
        if cap > 0.0 && (length as f32) > geometry.x(cap) {
            return Err(fail("MEDIA_INCOMPATIBLE", None,
                format!("automatic length reached {:.1} mm against a max_height_mm of {cap}", length as f32 * MM_PER_INCH / geometry.dpi_x)));
        }
        length
    };
    assert_length(profile, physical, length)?;
    Ok(length)
}

fn assert_length(profile: &Value, geometry: &Geometry, height: u32) -> Result<(), Fail> {
    let rules = &profile["length_rules"];
    let to_px = |um: f32| (um * geometry.dpi_y / 25400.0).round() as u32;
    match rules["kind"].as_str().unwrap_or_default() {
        "die_cut" => {
            let expected = to_px(number(rules, "fixed_um"));
            if height != expected {
                return Err(fail("MEDIA_INCOMPATIBLE", None, format!("die-cut media fixes the label at {expected} px and the layout is {height} px")));
            }
        }
        _ => {
            let (min, max) = (to_px(number(rules, "min_um")), to_px(number(rules, "max_um")));
            if height < min || height > max {
                return Err(fail("MEDIA_INCOMPATIBLE", None, format!("the layout is {height} px long and this media permits {min} to {max}")));
            }
        }
    }
    Ok(())
}

/// The last pixel row a QR element occupies, its trailing quiet zone included.
fn qr_bottom(geometry: &Geometry, element: &Value, input: &Value) -> u32 {
    let id = element["id"].as_str().unwrap_or("qr");
    let size = input["qr"][id]["modules"].as_array().map_or(0, Vec::len) as u32;
    let module_y = geometry.y(number(element, "module_mm")).max(1.0) as u32;
    let quiet = element["quiet_zone_modules"].as_u64().unwrap_or(4) as u32;
    geometry.y(number(element, "y_mm")) as u32 + (size + 2 * quiet) * module_y
}

/// The last pixel column a QR element occupies, its trailing quiet zone included.
fn qr_right(geometry: &Geometry, element: &Value, input: &Value) -> u32 {
    let id = element["id"].as_str().unwrap_or("qr");
    let size = input["qr"][id]["modules"].as_array().map_or(0, Vec::len) as u32;
    let module_x = geometry.x(number(element, "module_mm")).max(1.0) as u32;
    let quiet = element["quiet_zone_modules"].as_u64().unwrap_or(4) as u32;
    geometry.x(number(element, "x_mm")) as u32 + (size + 2 * quiet) * module_x
}

fn paint_qr(pixmap: &mut tiny_skia::Pixmap, geometry: &Geometry, element: &Value, input: &Value, colour: [u8; 3]) -> Result<(), Fail> {
    let id = element["id"].as_str().unwrap_or("qr");
    // Victual computed this matrix from the payload it pinned. Encoding it here as well
    // would be a second implementation that could disagree with the first, and would make
    // Victual's re-encode verification a comparison of two guesses about the mask.
    let matrix = input["qr"].get(id).ok_or_else(|| fail("RENDER_FAILED", Some(id), "the request carries no module matrix for this element"))?;
    let modules: Vec<&str> = matrix["modules"].as_array()
        .ok_or_else(|| fail("RENDER_FAILED", Some(id), "the module matrix is not a list of rows"))?
        .iter().map(|r| r.as_str().unwrap_or("")).collect();

    let module_mm = number(element, "module_mm");
    let module_x = geometry.x(module_mm).max(1.0) as u32;
    let module_y = geometry.y(module_mm).max(1.0) as u32;
    if geometry.x(module_mm) < 1.0 || geometry.y(module_mm) < 1.0 {
        return Err(fail("QR_TOO_SMALL", Some(id), format!("a module of {module_mm} mm is under one device pixel on this profile")));
    }

    let quiet = element["quiet_zone_modules"].as_u64().unwrap_or(4) as u32;
    let origin_x = geometry.x(number(element, "x_mm")) as u32 + quiet * module_x;
    let origin_y = geometry.y(number(element, "y_mm")) as u32 + quiet * module_y;

    let size = modules.len() as u32;
    if origin_x + size * module_x > pixmap.width() || origin_y + size * module_y > pixmap.height() {
        return Err(fail("QR_TOO_SMALL", Some(id), "the symbol and its quiet zone do not fit the label"));
    }

    for (row, bits) in modules.iter().enumerate() {
        for (column, bit) in bits.bytes().enumerate() {
            if bit != b'1' {
                continue;
            }
            fill(pixmap,
                origin_x + column as u32 * module_x, origin_y + row as u32 * module_y,
                module_x, module_y, colour);
        }
    }
    Ok(())
}

fn paint_rect(pixmap: &mut tiny_skia::Pixmap, geometry: &Geometry, element: &Value, colour: [u8; 3], permits_red: bool) -> Result<(), Fail> {
    let x = geometry.x(number(element, "x_mm")) as u32;
    let y = geometry.y(number(element, "y_mm")) as u32;
    let w = geometry.x(number(element, "width_mm")) as u32;
    let h = geometry.y(number(element, "height_mm")) as u32;

    if let Some(fill_name) = element["fill"].as_str() {
        let fill_colour = named_colour(fill_name, permits_red)
            .map_err(|e| fail("MEDIA_INCOMPATIBLE", element["id"].as_str(), e))?;
        fill(pixmap, x, y, w, h, fill_colour);
    }
    let stroke = geometry.y(number(element, "stroke_mm")).round() as u32;
    if stroke > 0 {
        fill(pixmap, x, y, w, stroke, colour);
        fill(pixmap, x, y + h.saturating_sub(stroke), w, stroke, colour);
        fill(pixmap, x, y, stroke, h, colour);
        fill(pixmap, x + w.saturating_sub(stroke), y, stroke, h, colour);
    }
    Ok(())
}

fn paint_line(pixmap: &mut tiny_skia::Pixmap, geometry: &Geometry, element: &Value, colour: [u8; 3]) {
    let x1 = geometry.x(number(element, "x1_mm"));
    let y1 = geometry.y(number(element, "y1_mm"));
    let x2 = geometry.x(number(element, "x2_mm"));
    let y2 = geometry.y(number(element, "y2_mm"));
    let stroke = geometry.y(number(element, "stroke_mm")).round().max(1.0) as u32;

    let steps = ((x2 - x1).abs().max((y2 - y1).abs())).max(1.0) as u32;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = (x1 + (x2 - x1) * t).round() as u32;
        let y = (y1 + (y2 - y1) * t).round() as u32;
        fill(pixmap, x, y, stroke, stroke, colour);
    }
}

fn paint_image(pixmap: &mut tiny_skia::Pixmap, geometry: &Geometry, element: &Value, assets: &[(String, Vec<u8>)], colour: [u8; 3]) -> Result<(), Fail> {
    let id = element["id"].as_str().unwrap_or("image");
    let name = element["asset"].as_str().unwrap_or_default();
    let (_, bytes) = assets.iter().find(|(n, _)| n == name)
        .ok_or_else(|| fail("ASSET_UNAVAILABLE", Some(id), format!("no image asset named {name}")))?;

    let source = tiny_skia::Pixmap::decode_png(bytes)
        .map_err(|e| fail("ASSET_UNAVAILABLE", Some(id), format!("the asset does not decode: {e}")))?;

    let x = geometry.x(number(element, "x_mm")) as u32;
    let y = geometry.y(number(element, "y_mm")) as u32;
    let w = geometry.x(number(element, "width_mm")) as u32;
    let h = geometry.y(number(element, "height_mm")) as u32;

    // Nearest neighbour, deliberately. The output palette has two or three entries, so any
    // interpolation would invent colours the profile does not permit and the quantiser would
    // then have to guess at - and a smoothed edge on a two-bit label is not an improvement.
    for oy in 0..h {
        for ox in 0..w {
            let sx = (ox as f32 * source.width() as f32 / w as f32) as u32;
            let sy = (oy as f32 * source.height() as f32 / h as f32) as u32;
            let pixel = source.pixel(sx.min(source.width() - 1), sy.min(source.height() - 1));
            if let Some(pixel) = pixel {
                let luminance = 0.299 * pixel.red() as f32 + 0.587 * pixel.green() as f32 + 0.114 * pixel.blue() as f32;
                if pixel.alpha() > 127 && luminance < 128.0 {
                    fill(pixmap, x + ox, y + oy, 1, 1, colour);
                }
            }
        }
    }
    Ok(())
}

fn fill(pixmap: &mut tiny_skia::Pixmap, x: u32, y: u32, w: u32, h: u32, colour: [u8; 3]) {
    let width = pixmap.width();
    let height = pixmap.height();
    let pixels = pixmap.pixels_mut();
    let value = tiny_skia::ColorU8::from_rgba(colour[0], colour[1], colour[2], 255).premultiply();
    for dy in 0..h {
        for dx in 0..w {
            let (px, py) = (x + dx, y + dy);
            if px < width && py < height {
                pixels[(py * width + px) as usize] = value;
            }
        }
    }
}

/// Text as one SVG, inside an anisotropic scale so outlines are stretched rather than pixels.
fn text_svg(geometry: &Geometry, height: u32, elements: &[Value], laid_out: &[(usize, Vec<String>, f32, f32)], families: &[(String, String)], fields: &Value, permits_red: bool) -> Result<String, Fail> {
    let aniso = geometry.dpi_y / geometry.dpi_x;
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"##,
        w = geometry.width, h = height);

    for (index, lines, size_pt, spacing) in laid_out {
        let element = &elements[*index];
        let id = element["id"].as_str().unwrap_or("text");
        let asset = element["font_asset"].as_str().unwrap_or_default();
        let family = families.iter().find(|(n, _)| n == asset).map(|(_, f)| f.clone())
            .ok_or_else(|| fail("ASSET_UNAVAILABLE", Some(id), format!("no font asset named {asset}")))?;
        let colour = colour_of(element, "color", permits_red).map_err(|e| fail("MEDIA_INCOMPATIBLE", Some(id), e))?;
        let hex = format!("#{:02x}{:02x}{:02x}", colour[0], colour[1], colour[2]);

        let font_px = size_pt * geometry.dpi_x / POINTS_PER_INCH;
        let line_height = size_pt * geometry.dpi_y / POINTS_PER_INCH * spacing;
        let top = geometry.y(number(element, "y_mm"));
        let left = geometry.x(number(element, "x_mm"));
        let box_width = geometry.x(number(element, "width_mm"));
        let anchor = match element["align"].as_str().unwrap_or("left") {
            "center" => ("middle", left + box_width / 2.0),
            "right" => ("end", left + box_width),
            _ => ("start", left),
        };

        svg.push_str(&format!(
            r##"<g transform="scale(1,{aniso})" font-family="{family}" font-size="{font_px}" fill="{hex}" text-anchor="{}">"##,
            anchor.0));
        for (line_index, line) in lines.iter().enumerate() {
            let baseline = (top + line_height * (line_index as f32 + 1.0)) / aniso;
            svg.push_str(&format!(r##"<text x="{}" y="{baseline}">{}</text>"##, anchor.1, escape(line)));
        }
        svg.push_str("</g>");
    }

    let _ = fields;
    svg.push_str("</svg>");
    Ok(svg)
}

/// Thresholds the painted pixmap onto the profile's palette.
///
/// Nothing else in the pipeline is allowed to produce a colour the profile does not name, so
/// this is a check as much as a conversion: an off-palette pixel here would be a bug
/// upstream, and mapping it to the nearest entry keeps the artifact conformant while the
/// renderer's own tests catch the cause.
fn quantise(pixmap: &tiny_skia::Pixmap, palette: &[[u8; 3]], profile: &Value) -> Vec<u8> {
    let threshold = profile["pixel_policy"]["threshold"]["black_at_or_below"].as_f64().unwrap_or(127.0) as f32;
    let index_of = |colour: [u8; 3]| -> u8 {
        palette.iter().position(|c| *c == colour).unwrap_or(0) as u8
    };
    let white = index_of([0xff, 0xff, 0xff]);
    let black = index_of([0x00, 0x00, 0x00]);
    let red = palette.iter().position(|c| *c == [0xff, 0x00, 0x00]).map(|i| i as u8);

    pixmap.pixels().iter().map(|pixel| {
        let (r, g, b) = (pixel.demultiply().red(), pixel.demultiply().green(), pixel.demultiply().blue());
        if let Some(red) = red {
            if r > 128 && g < 128 && b < 128 {
                return red;
            }
        }
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        if luminance <= threshold { black } else { white }
    }).collect()
}

fn colour_of(element: &Value, key: &str, permits_red: bool) -> Result<[u8; 3], String> {
    named_colour(element[key].as_str().unwrap_or("black"), permits_red)
}

fn named_colour(name: &str, permits_red: bool) -> Result<[u8; 3], String> {
    match name {
        "black" => Ok([0, 0, 0]),
        "white" => Ok([0xff, 0xff, 0xff]),
        "red" if permits_red => Ok([0xff, 0, 0]),
        "red" => Err("the document paints red and this media profile is monochrome".into()),
        other => Err(format!("unknown colour {other}")),
    }
}

fn break_lines(text: &str, family: &str, font_px: f32, db: &fontdb::Database, box_width: f32, wrap: bool) -> Vec<String> {
    if !wrap {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let trial = if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
        if measure(&trial, family, font_px, db) <= box_width || current.is_empty() {
            current = trial;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn ellipsise(text: &str, family: &str, font_px: f32, db: &fontdb::Database, box_width: f32) -> Vec<String> {
    let mut kept: String = String::new();
    for character in text.chars() {
        let trial = format!("{kept}{character}\u{2026}");
        if measure(&trial, family, font_px, db) > box_width {
            break;
        }
        kept.push(character);
    }
    vec![format!("{kept}\u{2026}")]
}

/// The advance of the run, which is what line breaking compares against a box.
///
/// usvg reports a text node's bounding box as the run's advance rather than its ink extent -
/// established on 2026-09-07 by measuring six single glyphs against their `hmtx` advances,
/// which agreed exactly. Comparing advances against the box is correct; reading this as ink
/// bounds would make the overflow rule mean something else.
fn measure(text: &str, family: &str, font_px: f32, db: &fontdb::Database) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="1000"><text x="0" y="500" font-family="{family}" font-size="{font_px}">{}</text></svg>"##,
        escape(text));
    let mut options = usvg::Options::default();
    options.fontdb = std::sync::Arc::new(db.clone());
    match usvg::Tree::from_str(&svg, &options) {
        Ok(tree) => tree.root().abs_bounding_box().width(),
        Err(_) => 0.0,
    }
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn number(value: &Value, key: &str) -> f32 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32
}
