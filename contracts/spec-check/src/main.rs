//! `spec-check <contract.wasm>...`: exits non-zero if any wasm has no
//! contract spec or its spec references an undefined user-defined type.

use std::process::ExitCode;

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: spec-check <contract.wasm>...");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for path in &paths {
        let wasm = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("error: {path}: {e}");
                failed = true;
                continue;
            }
        };
        match spec_check::check_wasm(&wasm) {
            Ok(dangling) if dangling.is_empty() => println!("ok: {path}"),
            Ok(dangling) => {
                failed = true;
                for d in dangling {
                    eprintln!(
                        "error: {path}: {} references undefined type `{}` \
                         (a type alias in a spec position? write the primitive literally, DR-0032)",
                        d.site, d.name
                    );
                }
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
