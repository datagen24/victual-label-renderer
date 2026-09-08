//! One invocation, one render request, one exit.
//!
//! The workload scales to zero, so the dispatcher must find queued work while no renderer
//! exists and must tolerate a lost invocation. Both fall out of the request being durable:
//! this binary claims one, produces one artifact, submits it and exits, and nothing about
//! the queue depends on a live process remembering anything.
//!
//! It is given no printer address and no socket. That is what makes automatic retry of a
//! render safe while automatic redispatch of a delivery attempt is not
//! (ADR-0019 decision item 6): a retried render cannot print by construction rather than by
//! discipline.

mod api;
mod png;
mod render;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let mut base = String::new();
    let mut key_file = String::new();
    let mut key = std::env::var("VICTUAL_RENDERER_KEY").unwrap_or_default();
    let mut health = false;
    let mut once = true;
    let mut input_file = String::new();
    let mut out_file = String::new();

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            // A probe that contacts nothing. ADR-0010 rule 4 requires a probe the deployment
            // can execute against an image with no shell; an exec probe on the binary itself
            // is the cheapest thing that satisfies it, and it deliberately does not report on
            // Victual's health - a renderer marked unhealthy because the API is down would be
            // restarted for somebody else's outage.
            "--health" => health = true,
            "--drain" => once = false,
            // Offline mode: render one saved request document to a file and contact nothing.
            // It exists so the artifact this binary produces can be fed straight into
            // Victual's own verification in a test, which is the only way to establish that
            // the two agree about geometry rather than that each agrees with itself.
            "--input" => input_file = args.next().unwrap_or_default(),
            "--out" => out_file = args.next().unwrap_or_default(),
            "--api" => base = args.next().unwrap_or_default(),
            "--key-file" => key_file = args.next().unwrap_or_default(),
            "--version" => {
                println!("victual-label-renderer {VERSION}");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    if health {
        println!("{}", serde_json::json!({"ok": true, "component": "victual-label-renderer", "version": VERSION}));
        return;
    }

    if !input_file.is_empty() {
        std::process::exit(render_to_file(&input_file, &out_file));
    }

    if !key_file.is_empty() {
        match std::fs::read_to_string(&key_file) {
            Ok(contents) => key = contents.trim().to_string(),
            Err(error) => {
                eprintln!("cannot read {key_file}: {error}");
                std::process::exit(2);
            }
        }
    }
    if base.is_empty() || key.is_empty() {
        eprintln!("usage: victual-label-renderer --api <base url> --key-file <path> [--drain]");
        std::process::exit(2);
    }

    let api = api::Api::new(&base, &key, 30);
    let mut rendered = 0u32;

    loop {
        match run_one(&api) {
            Ok(true) => rendered += 1,
            Ok(false) => break,
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        if once {
            break;
        }
    }

    println!("{}", serde_json::json!({"ok": true, "rendered": rendered}));
}

fn run_one(api: &api::Api) -> Result<bool, String> {
    let input = match api.claim().map_err(|e| e.to_string())? {
        Some(input) => input,
        None => return Ok(false),
    };

    let request_id = input["render_request_id"].as_u64().unwrap_or_default();
    let token = input["generation_token"].as_str().unwrap_or_default().to_string();

    // Assets are fetched by id over the renderer's own authorized route. There is no field
    // in the request that names a destination, so there is nothing here to point anywhere.
    let mut assets = Vec::new();
    for asset in input["assets"].as_array().cloned().unwrap_or_default() {
        let id = asset["id"].as_u64().unwrap_or_default();
        let name = asset["name"].as_str().unwrap_or_default().to_string();
        match api.asset_bytes(id) {
            Ok(bytes) => assets.push((name, bytes)),
            Err(error) => {
                // An asset Victual cannot hand over is an input problem, not an
                // infrastructure one: retrying will fetch the same absence.
                let _ = api.invalid(request_id, &token, "ASSET_UNAVAILABLE", Some(&name), &error.to_string());
                return Ok(true);
            }
        }
    }

    match render::render(&input, &assets, VERSION) {
        Ok(output) => {
            if let Err(error) = api.submit(request_id, &token, &output.bytes, VERSION) {
                // Submitting is the one step here that can fail for a reason retrying might
                // fix, so it is reported as infrastructure rather than as a bad document.
                // The server bounds the retries; this process does not loop.
                let _ = api.failed(request_id, &token, &error.to_string());
                return Err(error.to_string());
            }
            println!("{}", serde_json::json!({
                "render_request_id": request_id, "width_px": output.width, "height_px": output.height,
                "byte_length": output.bytes.len()}));
            Ok(true)
        }
        Err(failure) => {
            api.invalid(request_id, &token, failure.code, failure.element.as_deref(), &failure.detail)
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

/// Renders a saved request document and writes the artifact, contacting nothing.
fn render_to_file(input_file: &str, out_file: &str) -> i32 {
    let text = match std::fs::read_to_string(input_file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {input_file}: {error}");
            return 2;
        }
    };
    let input: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{input_file} is not JSON: {error}");
            return 2;
        }
    };

    // Assets arrive beside the document as file paths, because there is no API to ask.
    let mut assets = Vec::new();
    for asset in input["assets"].as_array().cloned().unwrap_or_default() {
        let name = asset["name"].as_str().unwrap_or_default().to_string();
        if let Some(path) = asset["file"].as_str() {
            match std::fs::read(path) {
                Ok(bytes) => assets.push((name, bytes)),
                Err(error) => {
                    eprintln!("cannot read asset {path}: {error}");
                    return 2;
                }
            }
        }
    }

    match render::render(&input, &assets, VERSION) {
        Ok(output) => {
            if !out_file.is_empty() {
                if let Err(error) = std::fs::write(out_file, &output.bytes) {
                    eprintln!("cannot write {out_file}: {error}");
                    return 2;
                }
            }
            println!("{}", serde_json::json!({
                "ok": true, "width_px": output.width, "height_px": output.height,
                "byte_length": output.bytes.len()}));
            0
        }
        Err(failure) => {
            println!("{}", serde_json::json!({
                "ok": false, "code": failure.code, "element": failure.element, "detail": failure.detail}));
            3
        }
    }
}
