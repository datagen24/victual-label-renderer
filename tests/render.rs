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
