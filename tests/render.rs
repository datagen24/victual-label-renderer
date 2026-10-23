//! What this binary can be held to on its own.
//!
//! The checks that matter most are not here: whether the artifact agrees with Victual about
//! geometry is a question about two implementations, and a test inside one of them cannot
//! ask it. That one lives in Victual's suite as `.devtools/labels/renderer-agreement-tests.php`,
//! which runs this binary and feeds its output to the verifier that will accept or refuse it
//! in production. What is here is the half that is local: the form, the palette, and the
//! refusals that must happen before anything is drawn.

use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_victual-label-renderer")
}

fn run(input: &serde_json::Value, out: &str) -> (i32, serde_json::Value) {
    // Unique per call: cargo runs these in threads of one process, so a path derived from
    // the process id alone has every test writing over every other test's input.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("render-input-{}-{serial}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_string(input).unwrap()).unwrap();
    let output = Command::new(binary())
        .args(["--input", path.to_str().unwrap(), "--out", out])
        .output()
        .expect("renderer runs");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or(serde_json::json!({}));
    (output.status.code().unwrap_or(-1), report)
}

fn profile(color_mode: &str) -> serde_json::Value {
    let palette: Vec<&str> = if color_mode == "black_red" {
        vec!["#ffffff", "#000000", "#ff0000"]
    } else {
        vec!["#ffffff", "#000000"]
    };
    serde_json::json!({
        "id": 1, "version": 1, "contract_version": 1,
        "dpi_x": 300, "dpi_y": 300, "raster_width_px": 696,
        "color_mode": color_mode,
        "printable_width_um": 58928,
        "length_rules": {"kind": "endless", "min_um": 12700, "max_um": 254000},
        "feed_direction": "length",
        "pixel_policy": {"version": 1, "rounding": "nearest",
            "threshold": {"method": "luminance", "black_at_or_below": 127},
            "palette": palette},
        "transport_padding": {"leading_px": 0, "trailing_px": 0}
    })
}

/// A 21 x 21 matrix of alternating modules. Not a real symbol - this test is about where the
/// modules land and what colour they are, and Victual is the side that decides what they say.
fn matrix() -> serde_json::Value {
    let rows: Vec<String> = (0..21)
        .map(|row| (0..21).map(|column| if (row + column) % 2 == 0 { '1' } else { '0' }).collect())
        .collect();
    serde_json::json!({"modules": rows, "size": 21, "ec_level": "M", "payload": "VCTL:0ABCDEFGHJKMN"})
}

fn qr_document(module_mm: f64) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1, "entity_kind": "location",
        "canvas": {"width_mm": 58.9, "height_mm": 30.0, "max_height_mm": null,
                   "margins_mm": {"top": 1.0, "right": 1.0, "bottom": 1.0, "left": 1.0}},
        "elements": [{"type": "qr", "id": "code", "x_mm": 2.0, "y_mm": 2.0,
                      "module_mm": module_mm, "ec_level": "M", "quiet_zone_modules": 4,
                      "color": "black", "source": "label.payload"}]
    })
}

fn input(document: serde_json::Value, color_mode: &str) -> serde_json::Value {
    serde_json::json!({
        "render_request_id": 1, "generation": 1, "generation_token": "t", "purpose": "production",
        "template": {"template_id": 1, "template_version_id": 1, "document_digest": null},
        "document": document,
        "capture": {"id": 1, "fields": {"location.name": "Pantry"}, "locale": "en", "timezone": "UTC", "digest": "x"},
        "profile": profile(color_mode),
        "assets": [], "qr": {"code": matrix()},
        "expected_form": "raster/png-indexed;v=1"
    })
}

fn png_header(bytes: &[u8]) -> (u32, u32, u8, u8) {
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    (width, height, bytes[24], bytes[25])
}

#[test]
fn emits_indexed_png_at_the_profile_grid() {
    let out = std::env::temp_dir().join("render-form.png");
    let (code, report) = run(&input(qr_document(0.6), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");

    let bytes = std::fs::read(&out).unwrap();
    let (width, height, depth, colour) = png_header(&bytes);

    // The form identifier pins these two, not merely "PNG".
    assert_eq!(colour, 3, "colour type 3, indexed");
    assert_eq!(depth, 2, "bit depth 2");
    // fixed_grid: the raster is the profile's width, and the renderer scales nothing.
    assert_eq!(width, 696);
    assert_eq!(height, 354, "30 mm at 300 dpi");

    // The palette is the profile's, in the profile's order, and carries nothing else.
    let plte = bytes.windows(4).position(|w| w == b"PLTE").unwrap();
    let length = u32::from_be_bytes(bytes[plte - 4..plte].try_into().unwrap()) as usize;
    assert_eq!(length, 9, "three entries");
    assert_eq!(&bytes[plte + 4..plte + 13], &[0xff, 0xff, 0xff, 0, 0, 0, 0xff, 0, 0]);
}

#[test]
fn refuses_a_module_under_one_device_pixel() {
    let out = std::env::temp_dir().join("render-tiny.png");
    let (code, report) = run(&input(qr_document(0.001), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 3, "{report}");
    assert_eq!(report["code"], "QR_TOO_SMALL");
    assert_eq!(report["element"], "code");
}

#[test]
fn refuses_red_on_monochrome_media() {
    let mut document = qr_document(0.6);
    document["elements"][0]["color"] = serde_json::json!("red");
    let out = std::env::temp_dir().join("render-red.png");
    let (code, report) = run(&input(document, "monochrome"), out.to_str().unwrap());
    assert_eq!(code, 3, "{report}");
    assert_eq!(report["code"], "MEDIA_INCOMPATIBLE");
}

/// Issue #90's assertion, in the form this repository can make it.
///
/// The prototype handed its library an image authored against one dot count while the library
/// compared against another, so every endless print was silently resampled - twice, with
/// `dpi_600` set. There is no resize call here to count, because there is no resampling step
/// at all: the pixmap is allocated at the profile's grid and every element is placed into it.
/// So the assertion is that the artifact's dimensions are exactly the profile's, for both
/// media and at both resolutions, which is the property counting resize calls was a proxy for.
#[test]
fn never_resamples_at_either_resolution() {
    for (dpi_x, dpi_y, width) in [(300, 300, 696), (300, 600, 696), (600, 600, 1392)] {
        let mut request = input(qr_document(0.6), "black_red");
        request["profile"]["dpi_x"] = serde_json::json!(dpi_x);
        request["profile"]["dpi_y"] = serde_json::json!(dpi_y);
        request["profile"]["raster_width_px"] = serde_json::json!(width);

        let out = std::env::temp_dir().join(format!("render-{dpi_x}x{dpi_y}.png"));
        let (code, report) = run(&request, out.to_str().unwrap());
        assert_eq!(code, 0, "{dpi_x}x{dpi_y}: {report}");

        let bytes = std::fs::read(&out).unwrap();
        let (actual_width, actual_height, _, _) = png_header(&bytes);
        assert_eq!(actual_width, width, "{dpi_x}x{dpi_y} width is the profile's, not a resample of it");
        assert_eq!(actual_height, (30.0 * dpi_y as f64 / 25.4).round() as u32, "{dpi_x}x{dpi_y} height");
    }
}

/// Issue #948: automatic height measured text, rects and images but skipped the QR, so a
/// QR-only label came out as long as its bottom margin (24 px) and the media refused it.
#[test]
fn automatic_height_holds_the_whole_qr_symbol() {
    let mut document = qr_document(0.5);
    document["canvas"]["height_mm"] = serde_json::Value::Null;
    document["canvas"]["max_height_mm"] = serde_json::json!(100.0);
    let out = std::env::temp_dir().join("render-auto-qr.png");
    let (code, report) = run(&input(document, "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");

    let (_, height, _, _) = png_header(&std::fs::read(&out).unwrap());
    // Nearest-pixel rounding: y 2 mm = 24 px; 21 modules and two 4-module quiet zones at
    // 6 px each = 174 px; the 1 mm bottom margin adds 12 px to the 198 px extent.
    assert_eq!(height, 210);
}

/// Whether the pixel at (x, y) of a rendered artifact is black.
fn dark_at(path: &std::path::Path, x: u32, y: u32) -> bool {
    let pixmap = tiny_skia::Pixmap::load_png(path).expect("the artifact decodes");
    let pixel = pixmap.pixel(x, y).expect("inside the artifact");
    pixel.red() < 128 && pixel.green() < 128
}

fn turned(mut document: serde_json::Value, degrees: u32) -> serde_json::Value {
    document["schema_version"] = serde_json::json!(2);
    document["canvas"]["orientation_deg"] = serde_json::json!(degrees);
    document
}

/// #1033: the reading frame is turned clockwise onto the raster by an exact pixel permutation.
///
/// The QR's first module (row 0, column 0) is dark and its neighbour in column 1 is light.
/// At 300 dpi the module is 7 px and the symbol starts 24 + 4 * 7 = 52 px in, so the two
/// module centres sit at reading (55, 55) and (62, 55). A 90 degree turn of a reading image
/// 696 px tall puts reading (x, y) at physical (695 - y, x).
#[test]
fn a_quarter_turn_lands_each_module_where_the_convention_says() {
    let out = std::env::temp_dir().join("render-turn-0.png");
    let (code, report) = run(&input(qr_document(0.6), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    assert!(dark_at(&out, 55, 55) && !dark_at(&out, 62, 55), "unturned, the modules are where they were drawn");

    let out = std::env::temp_dir().join("render-turn-90.png");
    let (code, report) = run(&input(turned(qr_document(0.6), 90), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    let (width, height, _, _) = png_header(&std::fs::read(&out).unwrap());
    assert_eq!((width, height), (696, 354), "the raster is still the roll's width by the 30 mm length");
    assert!(dark_at(&out, 640, 55), "reading (55, 55) lands at physical (640, 55)");
    assert!(!dark_at(&out, 640, 62), "reading (62, 55), the light neighbour, lands at physical (640, 62)");

    let out = std::env::temp_dir().join("render-turn-270.png");
    let (code, report) = run(&input(turned(qr_document(0.6), 270), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    // 270: reading (x, y) lands at physical (y, W - 1 - x), with W the 354 px reading width.
    assert!(dark_at(&out, 55, 298) && !dark_at(&out, 55, 291), "the opposite quarter turn mirrors it");
}

/// On a 300 x 600 device, a quarter-turned module is 7 px across the roll and 14 px along the
/// tape: each reading axis takes the dpi of the physical axis it becomes, so the symbol is
/// never stretched the wrong way.
#[test]
fn a_quarter_turn_keeps_each_physical_axis_at_its_own_dpi() {
    let mut request = input(turned(qr_document(0.6), 90), "black_red");
    request["profile"]["dpi_y"] = serde_json::json!(600);
    let out = std::env::temp_dir().join("render-turn-aniso.png");
    let (code, report) = run(&request, out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");

    let (width, height, _, _) = png_header(&std::fs::read(&out).unwrap());
    assert_eq!((width, height), (696, 709), "30 mm along the tape at 600 dpi");
    // Reading x now runs at 600 dpi: x 2 mm = 47 px, module 14 px, origin 47 + 56 = 103,
    // column 0 centre 110 and column 1 centre 124. Reading y runs at 300 dpi: centre 55.
    assert!(dark_at(&out, 640, 110), "column 0 is 14 px tall along the tape");
    assert!(!dark_at(&out, 640, 124), "column 1 starts 14 px later");
}

/// Automatic length on a turned design is the content's reading width.
#[test]
fn automatic_length_on_a_quarter_turn_is_the_reading_width() {
    let mut document = turned(qr_document(0.6), 90);
    document["canvas"]["height_mm"] = serde_json::Value::Null;
    document["canvas"]["max_height_mm"] = serde_json::json!(100.0);
    let out = std::env::temp_dir().join("render-turn-auto.png");
    let (code, report) = run(&input(document, "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    let (width, height, _, _) = png_header(&std::fs::read(&out).unwrap());
    // 24 px + 29 modules * 7 px = 227 px, plus the 1 mm (12 px) right margin.
    assert_eq!((width, height), (696, 239));
}

/// An orientation is a schema 2 feature: a version 1 document carrying one is refused.
#[test]
fn orientation_needs_schema_version_two() {
    let mut document = qr_document(0.6);
    document["canvas"]["orientation_deg"] = serde_json::json!(90);
    let out = std::env::temp_dir().join("render-turn-v1.png");
    let (code, report) = run(&input(document, "black_red"), out.to_str().unwrap());
    assert_eq!(code, 3, "{report}");
    assert_eq!(report["code"], "UNSUPPORTED_VERSION");
}

fn with_rotation(mut document: serde_json::Value, index: usize, degrees: u32) -> serde_json::Value {
    document["schema_version"] = serde_json::json!(2);
    document["elements"][index]["rotation_deg"] = serde_json::json!(degrees);
    document
}

/// #1033 phase 2: a turned element is painted in its own frame, turned clockwise by the same
/// permutation as a canvas, and placed with its footprint's top-left at (x_mm, y_mm).
///
/// The 29-module symbol is 203 px square at 7 px a module. Unturned in its own frame, the
/// first module's centre is (31, 31) and its light neighbour's (38, 31). Turned 90 they land
/// at (202 - 31, 31) = (171, 31) and (171, 38), plus the 24 px footprint origin.
#[test]
fn a_turned_qr_lands_where_the_convention_says() {
    let out = std::env::temp_dir().join("render-element-90.png");
    let (code, report) = run(&input(with_rotation(qr_document(0.6), 0, 90), "black_red"), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    assert!(dark_at(&out, 195, 55), "local (31, 31) lands at (195, 55)");
    assert!(!dark_at(&out, 195, 62), "local (38, 31), the light neighbour, lands at (195, 62)");
}

/// A turned text box keeps its ink inside its footprint, which is its box with width and
/// height swapped: a 20 x 6 mm box turned 90 at (2, 2) covers x 24..95 and y 24..260.
#[test]
fn turned_text_stays_inside_its_footprint() {
    let font = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/NotoSans-Regular.ttf");
    let mut document = qr_document(0.6);
    document["schema_version"] = serde_json::json!(2);
    document["elements"] = serde_json::json!([{
        "type": "text", "id": "name", "x_mm": 2.0, "y_mm": 2.0, "width_mm": 20.0, "height_mm": 6.0,
        "field": "location.name", "font_asset": "fixture", "size_pt": 11.0, "align": "left",
        "valign": "top", "wrap": true, "line_spacing": 1.2, "overflow": "error", "color": "black",
        "rotation_deg": 90}]);
    let mut request = input(document, "black_red");
    request["assets"] = serde_json::json!([{"id": 1, "name": "fixture", "file": font}]);
    let out = std::env::temp_dir().join("render-element-text.png");
    let (code, report) = run(&request, out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");

    let pixmap = tiny_skia::Pixmap::load_png(&out).unwrap();
    let (mut inside, mut outside) = (0, 0);
    for y in 0..pixmap.height() {
        for x in 0..pixmap.width() {
            if pixmap.pixel(x, y).unwrap().red() < 128 {
                if (24..95).contains(&x) && (24..260).contains(&y) { inside += 1 } else { outside += 1 }
            }
        }
    }
    assert!(inside > 50, "the text was drawn ({inside} dark pixels)");
    assert_eq!(outside, 0, "no ink outside the turned footprint");
}

/// rotation_deg is schema 2, a quarter turn, and not on a line.
#[test]
fn element_rotation_is_refused_where_it_does_not_apply() {
    let mut v1 = qr_document(0.6);
    v1["elements"][0]["rotation_deg"] = serde_json::json!(90);
    let out = std::env::temp_dir().join("render-element-refused.png");
    let (code, report) = run(&input(v1, "black_red"), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str()), (3, Some("UNSUPPORTED_VERSION")), "{report}");

    let (code, report) = run(&input(with_rotation(qr_document(0.6), 0, 45), "black_red"), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str()), (3, Some("UNSUPPORTED_VERSION")), "{report}");
}

/// A text request on the QR fixture's canvas, holding one text box at (2, 2) mm with the given
/// value, height and overflow rule; the box is 30 mm wide and 9 pt, as on the 2026-10-23
/// stock label.
fn text_request(value: &str, height_mm: f64, overflow: &str, extra: serde_json::Value) -> serde_json::Value {
    let font = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/NotoSans-Regular.ttf");
    let mut document = qr_document(0.6);
    let mut element = serde_json::json!({
        "type": "text", "id": "name", "x_mm": 2.0, "y_mm": 2.0, "width_mm": 30.0, "height_mm": height_mm,
        "field": "location.name", "font_asset": "fixture", "size_pt": 9.0, "align": "left",
        "valign": "top", "wrap": true, "line_spacing": 1.2, "overflow": overflow, "color": "black"});
    for (key, value) in extra.as_object().unwrap() {
        element[key] = value.clone();
    }
    document["elements"] = serde_json::json!([element]);
    let mut request = input(document, "black_red");
    request["capture"]["fields"]["location.name"] = serde_json::json!(value);
    request["assets"] = serde_json::json!([{"id": 1, "name": "fixture", "file": font}]);
    request
}

/// Dark pixels below a row, which is where text that ran out of its box lands.
fn ink_below(path: &std::path::Path, row: u32) -> usize {
    let pixmap = tiny_skia::Pixmap::load_png(path).unwrap();
    let mut count = 0;
    for y in row..pixmap.height() {
        for x in 0..pixmap.width() {
            if pixmap.pixel(x, y).unwrap().red() < 128 {
                count += 1;
            }
        }
    }
    count
}

/// Capitals without descenders, long enough to need five lines, 19 mm, in a 30 mm box at 9 pt.
const LONG: &str = "GREAT VALUE ORGANIC WHOLE PEELED TOMATOES IN TOMATO HEAT AND BASIL";

/// The bottom of the (2 mm, 2 + h mm) box at 300 dpi, plus a pixel for rounding.
fn bottom_px(height_mm: f32) -> u32 {
    ((2.0 + height_mm) * 300.0 / 25.4).round() as u32 + 1
}

#[test]
fn overflow_error_refuses_text_taller_than_its_box() {
    let out = std::env::temp_dir().join("render-overflow-error.png");
    // 2026-10-23: a value needing several lines in a 3.2 mm box were drawn over the element below.
    let (code, report) = run(&text_request(LONG, 3.2, "error", serde_json::json!({})), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str(), report["element"].as_str()), (3, Some("TEXT_OVERFLOW"), Some("name")), "{report}");
    assert!(report["detail"].as_str().unwrap().contains("3.2 mm tall"), "{report}");

    // One 9 pt line at 1.2 spacing advances 3.81 mm, so even a short value needs a taller box.
    let (code, report) = run(&text_request("SALT", 3.2, "error", serde_json::json!({})), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str()), (3, Some("TEXT_OVERFLOW")), "{report}");

    let (code, report) = run(&text_request(LONG, 20.0, "error", serde_json::json!({})), out.to_str().unwrap());
    assert_eq!(code, 0, "five lines fit 20 mm: {report}");
    assert_eq!(ink_below(&out, bottom_px(20.0)), 0, "no ink below the box");
}

#[test]
fn ellipsis_keeps_the_lines_that_fit_and_ends_the_last() {
    let out = std::env::temp_dir().join("render-overflow-ellipsis.png");
    let (code, report) = run(&text_request(LONG, 8.0, "ellipsis", serde_json::json!({})), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    assert!(ink_below(&out, 2 * 24) > 0, "text was drawn");
    assert_eq!(ink_below(&out, bottom_px(8.0)), 0, "two lines kept, nothing below the 8 mm box");

    // Not even one line fits: there is nothing to put an ellipsis on.
    let (code, report) = run(&text_request(LONG, 3.0, "ellipsis", serde_json::json!({})), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str()), (3, Some("TEXT_OVERFLOW")), "{report}");
}

#[test]
fn shrink_to_fit_shrinks_for_height_as_well_as_width() {
    let out = std::env::temp_dir().join("render-overflow-shrink.png");
    let (code, report) = run(&text_request(LONG, 8.0, "shrink_to_fit", serde_json::json!({"min_size_pt": 4.0})), out.to_str().unwrap());
    assert_eq!(code, 0, "{report}");
    assert_eq!(ink_below(&out, bottom_px(8.0)), 0, "shrunk until it fits the 8 mm box");

    let (code, report) = run(&text_request(LONG, 8.0, "shrink_to_fit", serde_json::json!({"min_size_pt": 8.5})), out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str(), report["element"].as_str()), (3, Some("TEXT_OVERFLOW"), Some("name")), "a floor that still overflows refuses: {report}");
}

#[test]
fn turned_text_follows_the_same_overflow_rules() {
    let out = std::env::temp_dir().join("render-overflow-turned.png");
    let mut request = text_request(LONG, 3.2, "error", serde_json::json!({"rotation_deg": 90}));
    request["document"]["schema_version"] = serde_json::json!(2);
    let (code, report) = run(&request, out.to_str().unwrap());
    assert_eq!((code, report["code"].as_str(), report["element"].as_str()), (3, Some("TEXT_OVERFLOW"), Some("name")), "{report}");

    let mut request = text_request(LONG, 8.0, "ellipsis", serde_json::json!({"rotation_deg": 90}));
    request["document"]["schema_version"] = serde_json::json!(2);
    let (code, report) = run(&request, out.to_str().unwrap());
    assert_eq!(code, 0, "a turned box ellipsises too: {report}");
}
