//! Shared integration-test helpers: a fake `herdr-axi` executable and scratch
//! plumbing. Each `tests/*.rs` binary imports this as `mod common;`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Build a fake `herdr-axi` executable in `dir`. On invocation it appends the
/// received `--request-file` envelope (one JSON document per line) to `capture`,
/// then prints a canned `herdr-axi --json` response carrying `disposition`.
pub fn fake_herdr_axi(dir: &Path, disposition: &str, capture: &Path) -> PathBuf {
    let bin = dir.join("herdr-axi");
    let response = match disposition {
        "submitted" => r#"{"disposition":"submitted","result":{"state":"idle"}}"#,
        "not-submitted" => {
            r#"{"disposition":"not-submitted","result":{"reason":"controlled by someone","state":"busy"}}"#
        }
        "unknown" => r#"{"disposition":"unknown","result":{"state":"unknown"}}"#,
        other => panic!("unknown fake disposition: {other}"),
    };
    let script = format!(
        "#!/bin/sh\n\
while [ $# -gt 0 ]; do\n\
  case \"$1\" in\n\
    --request-file) req=\"$2\"; shift 2 ;;\n\
    *) shift ;;\n\
  esac\n\
done\n\
cat \"$req\" >> \"{capture}\"\n\
printf '\\n' >> \"{capture}\"\n\
printf '%s\\n' '{response}'\n",
        capture = capture.display(),
        response = response,
    );
    std::fs::write(&bin, script).expect("write fake herdr-axi");
    set_executable(&bin);
    bin
}

fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .expect("fake bin metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod +x fake herdr-axi");
}

/// A fresh scratch directory for one test, unique per process + call.
pub fn scratch_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("herdr-board-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Parse every envelope line a fake `herdr-axi` captured, in order.
pub fn captured_envelopes(capture: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(capture).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("captured envelope is valid JSON"))
        .collect()
}
