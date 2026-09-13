//! Runnable smoke of G5: draft an ordinary card and a factory card (factory
//! kind fixed at draft creation), promote the ordinary card to a factory
//! request, and show the resulting request's kind + digest — and that a
//! re-promote is refused.

use std::path::PathBuf;

use herdr_board::{create_draft, promote_to_factory_request, FactoryKind, Store};

fn main() {
    let dir = std::env::temp_dir().join(format!("herdr-board-factory-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path: PathBuf = dir.join("demo.sqlite3");
    let store = Store::open(&path).expect("open store");

    // 1. An ordinary card — factory_kind persisted atomically at draft time.
    let ordinary = create_draft(&store, FactoryKind::Ordinary, "Fix the board sync", "body")
        .expect("create ordinary draft");
    println!(
        "draft {} {}",
        ordinary.draft_id,
        ordinary.factory_kind.as_str()
    );

    // 2. A factory card — its kind is fixed at draft creation, never attached later.
    let factory = create_draft(
        &store,
        FactoryKind::FactoryRequest,
        "Process with factory",
        "body",
    )
    .expect("create factory draft");
    println!(
        "draft {} {}",
        factory.draft_id,
        factory.factory_kind.as_str()
    );

    // 3. Promote the ordinary card: the single explicit transition.
    let request = promote_to_factory_request(&store, &ordinary.draft_id).expect("promote");
    println!(
        "request factory-kind: {}, digest id: {}",
        request.factory_kind.as_str(),
        request.digest.digest_id()
    );

    // 4. Re-promote is refused (monotonic; never auto-fires).
    match promote_to_factory_request(&store, &ordinary.draft_id) {
        Err(e) => println!("re-promote refused: {e}"),
        Ok(_) => {
            eprintln!("expected re-promote refusal");
            std::process::exit(1);
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
