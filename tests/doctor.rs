//! `ctxgrep doctor` and the install path, run against the built binary with a made-up transcript folder.
use std::path::PathBuf;
use std::process::{Command, Output};

/// A fresh, empty folder under target/tmp (never /tmp).
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctxgrep(src: &PathBuf, db: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctxgrep")).args(args).env("CTXGREP_SRC", src).env("CTXGREP_DB", db).output().unwrap()
}

fn text(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr) }

const SESSION: &str = r#"{"type":"user","sessionId":"aaaaaaaa-1111","timestamp":"2026-11-10T15:00:00.000Z","cwd":"/work/shop","uuid":"u1","message":{"role":"user","content":"why did the checkout page return a 500 after the juniper upgrade?"}}
{"type":"assistant","sessionId":"aaaaaaaa-1111","timestamp":"2026-11-10T15:00:30.000Z","uuid":"u2","message":{"role":"assistant","model":"m","content":[{"type":"text","text":"The juniper-session cookie was renamed in 4.2, so the old name no longer matches."}]}}
"#;

fn with_transcript(name: &str) -> (PathBuf, PathBuf) {
    let root = scratch(name);
    let src = root.join("projects/-work-shop");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("aaaaaaaa-1111.jsonl"), SESSION).unwrap();
    (root.join("projects"), root.join("store/ctxgrep.db"))
}

#[test]
fn doctor_is_clean_after_an_index() {
    let (src, db) = with_transcript("doctor-ok");
    assert!(ctxgrep(&src, &db, &["index"]).status.success());
    let o = ctxgrep(&src, &db, &["doctor"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert_eq!(t.lines().count(), 4, "one line per check: {t}");
    assert!(t.lines().all(|l| l.starts_with("ok")), "{t}");
}

#[test]
fn doctor_says_to_index_when_the_store_is_empty() {
    let (src, db) = with_transcript("doctor-empty");
    let o = ctxgrep(&src, &db, &["doctor"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("FAIL  index") && t.contains("ctxgrep index"), "{t}");
    assert!(t.contains("ok    source"), "{t}");
}

#[test]
fn doctor_fails_on_a_missing_source_folder() {
    let root = scratch("doctor-nosrc");
    let o = ctxgrep(&root.join("nope"), &root.join("store/ctxgrep.db"), &["doctor"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("FAIL  source") && t.contains("CTXGREP_SRC"), "{t}");
}

#[test]
fn doctor_fails_on_a_source_folder_with_no_transcripts() {
    let root = scratch("doctor-notranscripts");
    let src = root.join("projects");
    std::fs::create_dir_all(&src).unwrap();
    let o = ctxgrep(&src, &root.join("store/ctxgrep.db"), &["doctor"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("FAIL  source") && t.contains("no .jsonl"), "{t}");
}

#[test]
fn doctor_fails_when_the_store_cannot_be_written() {
    let (src, _) = with_transcript("doctor-readonly");
    // A file where the store's folder should be: creating the folder fails, whoever runs the test.
    let blocker = src.parent().unwrap().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let o = ctxgrep(&src, &blocker.join("ctxgrep.db"), &["doctor"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("FAIL  store") && t.contains("CTXGREP_DB"), "{t}");
}

#[test]
fn search_finds_the_made_up_session() {
    let (src, db) = with_transcript("search");
    assert!(ctxgrep(&src, &db, &["index"]).status.success());
    let o = ctxgrep(&src, &db, &["juniper"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("aaaaaaaa:") && t.contains("[juniper]"), "{t}");
}

#[test]
fn version_runs() {
    let (src, db) = with_transcript("version");
    let o = ctxgrep(&src, &db, &["--version"]);
    assert!(o.status.success() && text(&o).starts_with("ctxgrep "), "{}", text(&o));
}
