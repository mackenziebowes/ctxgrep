//! ctxgrep: grep over every Claude Code session on this machine. One SQLite store (spans + FTS5) built from the raw
//! transcripts in ~/.claude/projects, which stay the source of truth; `ctxgrep rebuild` makes the store again from them.
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Longest text kept per span, in bytes; `bytes` keeps the real length. A choice for today, to keep the store small.
const MAX_TEXT: usize = 100_000;

const SCHEMA: &str = "
create table if not exists sessions(
  session_id text primary key, agent text, model text, cwd text,
  started_at text, ended_at text, file text, parent_session text);
create table if not exists spans(
  id integer primary key, session_id text not null, ts text, turn integer not null, uuid text, parent_uuid text,
  role text not null, tool text, use_id text, text text not null, bytes integer not null);
create index if not exists spans_session on spans(session_id, turn);
create index if not exists spans_use on spans(use_id);
create index if not exists spans_ts on spans(ts);
create virtual table if not exists spans_fts using fts5(text, content='spans', content_rowid='id');
create trigger if not exists spans_ai after insert on spans begin
  insert into spans_fts(rowid, text) values (new.id, new.text); end;
create table if not exists offsets(file text primary key, bytes_read integer not null, lines integer not null);
";

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").expect("HOME is not set")) }
fn db_path() -> PathBuf {
    std::env::var("CTXGREP_DB").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share/ctxgrep/ctxgrep.db"))
}
fn src_dir() -> PathBuf {
    std::env::var("CTXGREP_SRC").map(PathBuf::from).unwrap_or_else(|_| home().join(".claude/projects"))
}

fn open() -> Connection {
    let p = db_path();
    std::fs::create_dir_all(p.parent().unwrap()).expect("make the store's folder");
    let c = Connection::open(&p).unwrap_or_else(|e| die(&format!("open {}: {e}", p.display())));
    c.execute_batch("pragma journal_mode=wal; pragma synchronous=normal; pragma busy_timeout=5000;").unwrap();
    c.execute_batch(SCHEMA).unwrap();
    c
}

fn die(msg: &str) -> ! { eprintln!("ctxgrep: {msg}"); std::process::exit(2) }

fn main() {
    // Stop quietly when the reader goes away (ctxgrep ... | head), as grep does.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => print!("{HELP}"),
        Some("index") => index(&open()),
        Some("rebuild") => {
            let p = db_path();
            for f in [p.clone(), p.with_extension("db-wal"), p.with_extension("db-shm")] { let _ = std::fs::remove_file(f); }
            index(&open())
        }
        Some("sql") => sql(&args[1..]),
        Some("session") => session(&args[1..]),
        Some("last") => last(&args[1..]),
        _ => search(&args),
    }
}

const HELP: &str = "ctxgrep: grep over every Claude Code session on this machine (all agents, tool output included)

  ctxgrep [options] <words>        search; newest first, one hit per line:
                                   <local time>  <agent>  <session>:<turn>  <role>[tool]  <text>
  ctxgrep last [agent]             your previous session's talk, in order: the first call after a clear
  ctxgrep session <session|agent> [--from TURN] [--to TURN] [--talk]
                                   read one session in order; an agent name means its newest session
  ctxgrep sql \"<select ...>\"       read-only SQL over sessions and spans (tab-separated out)
  ctxgrep index | rebuild          read what is new in ~/.claude/projects | drop the store and read it all

Words use full-text syntax: juniper desk (both), \"exact phrase\", juni*, a OR b, a NOT b, NEAR(a b, 10).

Options:
  -E               the pattern is a regular expression, not words (scans text; narrow it with the filters below)
  -i               case-insensitive regex (word search already is)
  -C <N>           show N turns before and after each hit, from the same session
  -l               list matching sessions instead of hits
  -c               count matching turns
  -m <N>           at most N hits (default 20)
  --agent <name>   one agent's sessions          --session <id>   one session (a prefix is enough)
  --role <role>    user | assistant | tool_use | tool_result | system
  --tool <name>    Bash, Edit, Read, Write, ... (tool_use and tool_result turns)
  --cwd <text>     sessions whose working folder contains this text
  --since <time>   at or after;  --until <time>  before
                   a time is 30m, 2h, 3d or '2 hours ago'; today, yesterday; 15:30 or 2026-11-18 15:30 (local);
                   or ISO with Z or an offset. All times shown are local.

Exit 0 when something matched, 1 when nothing did (stderr says whether the words or the filters ruled it out), 2 on an error.
Store: $CTXGREP_DB or ~/.local/share/ctxgrep/ctxgrep.db.  Source: $CTXGREP_SRC or ~/.claude/projects.
";

// ---------- index ----------

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { jsonl_files(&p, out) } else if p.extension().is_some_and(|x| x == "jsonl") { out.push(p) }
    }
}

fn index(c: &Connection) {
    let mut files = Vec::new();
    jsonl_files(&src_dir(), &mut files);
    let (mut n_files, mut n_spans) = (0, 0usize);
    for f in &files {
        match index_file(c, f) { Ok(0) => {}, Ok(n) => { n_files += 1; n_spans += n } Err(e) => eprintln!("ctxgrep: {}: {e}", f.display()) }
    }
    println!("indexed {n_spans} spans from {n_files} changed files ({} files seen)", files.len());
}

fn index_file(c: &Connection, f: &Path) -> Result<usize, String> {
    let key = f.to_string_lossy().to_string();
    let len = std::fs::metadata(f).map_err(|e| e.to_string())?.len();
    let (from, mut line_no): (u64, i64) = c
        .query_row("select bytes_read, lines from offsets where file=?", [&key], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))
        .optional().map_err(|e| e.to_string())?.unwrap_or((0, 0));
    if len <= from { return Ok(0) }
    let mut fh = std::fs::File::open(f).map_err(|e| e.to_string())?;
    fh.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
    let mut rd = BufReader::new(fh.take(len - from));
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    let (mut read, mut n) = (from, 0usize);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let got = rd.read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
        if got == 0 || buf.last() != Some(&b'\n') { break } // a half-written last line waits for the next run
        read += got as u64;
        line_no += 1;
        if let Ok(v) = serde_json::from_slice::<Value>(&buf) { n += take_line(&tx, f, line_no, &v)? }
    }
    tx.execute("insert into offsets(file, bytes_read, lines) values(?,?,?) on conflict(file) do update set bytes_read=excluded.bytes_read, lines=excluded.lines",
        params![key, read as i64, line_no]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

fn s(v: &Value, k: &str) -> Option<String> { v.get(k).and_then(Value::as_str).map(str::to_string) }

fn clip(t: String) -> (String, i64) {
    let b = t.len() as i64;
    if t.len() <= MAX_TEXT { return (t, b) }
    let mut cut = MAX_TEXT;
    while !t.is_char_boundary(cut) { cut -= 1 }
    (format!("{}\n[trace: cut at {MAX_TEXT} of {b} bytes]", &t[..cut]), b)
}

fn content_text(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn take_line(c: &Connection, f: &Path, turn: i64, v: &Value) -> Result<usize, String> {
    let Some(sid) = s(v, "sessionId") else { return Ok(0) };
    let ty = s(v, "type").unwrap_or_default();
    let ts = s(v, "timestamp");
    c.execute("insert into sessions(session_id, file, started_at, ended_at) values(?1,?2,?3,?3) on conflict(session_id) do nothing",
        params![sid, f.to_string_lossy(), ts]).map_err(|e| e.to_string())?;
    if let Some(t) = &ts {
        c.execute("update sessions set started_at=coalesce(min(started_at,?2),?2), ended_at=coalesce(max(ended_at,?2),?2) where session_id=?1",
            params![sid, t]).map_err(|e| e.to_string())?;
    }
    if ty == "agent-name" {
        if let Some(a) = s(v, "agentName") { c.execute("update sessions set agent=? where session_id=?", params![a, sid]).map_err(|e| e.to_string())?; }
        return Ok(0);
    }
    if let Some(cwd) = s(v, "cwd") { c.execute("update sessions set cwd=coalesce(cwd,?) where session_id=?", params![cwd, sid]).map_err(|e| e.to_string())?; }
    let msg = v.get("message");
    if ty == "assistant" {
        if let Some(m) = msg.and_then(|m| s(m, "model")) { c.execute("update sessions set model=? where session_id=?", params![m, sid]).map_err(|e| e.to_string())?; }
    }
    let (uuid, parent) = (s(v, "uuid"), s(v, "parentUuid"));
    let put = |role: &str, tool: Option<String>, use_id: Option<String>, text: String| -> Result<usize, String> {
        if text.trim().is_empty() { return Ok(0) }
        let (text, bytes) = clip(text);
        c.execute("insert into spans(session_id, ts, turn, uuid, parent_uuid, role, tool, use_id, text, bytes) values(?,?,?,?,?,?,?,?,?,?)",
            params![sid, ts, turn, uuid, parent, role, tool, use_id, text, bytes]).map_err(|e| e.to_string())?;
        Ok(1)
    };
    let mut n = 0;
    match ty.as_str() {
        "user" => match msg.and_then(|m| m.get("content")) {
            Some(Value::Array(parts)) => for p in parts {
                if s(p, "type").as_deref() == Some("tool_result") {
                    let id = s(p, "tool_use_id");
                    let tool: Option<String> = id.as_ref().and_then(|i| c.query_row(
                        "select tool from spans where use_id=? and role='tool_use' limit 1", [i], |r| r.get(0)).optional().ok().flatten());
                    n += put("tool_result", tool, id, content_text(p.get("content").unwrap_or(&Value::Null)))?;
                } else if let Some(t) = p.get("text").and_then(Value::as_str) { n += put("user", None, None, t.to_string())? }
            },
            Some(other) => n += put("user", None, None, content_text(other))?,
            None => {}
        },
        "assistant" => if let Some(Value::Array(parts)) = msg.and_then(|m| m.get("content")) {
            for p in parts {
                match s(p, "type").as_deref() {
                    Some("text") => n += put("assistant", None, None, s(p, "text").unwrap_or_default())?,
                    Some("tool_use") => n += put("tool_use", s(p, "name"), s(p, "id"), p.get("input").map(Value::to_string).unwrap_or_default())?,
                    _ => {} // thinking is left out
                }
            }
        },
        "system" => n += put("system", None, None, s(v, "content").unwrap_or_default())?,
        _ => {}
    }
    Ok(n)
}


// ---------- queries ----------

fn ro(case_insensitive: bool) -> Connection {
    let p = db_path();
    if !p.exists() { die(&format!("no store at {}; run `ctxgrep index` first", p.display())) }
    let c = Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap_or_else(|e| die(&e.to_string()));
    c.execute_batch("pragma busy_timeout=5000;").unwrap();
    // regexp(pattern, text): compiled once per statement and cached by SQLite as auxiliary data.
    c.create_scalar_function("regexp", 2, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, move |ctx| {
        let re = ctx.get_or_create_aux(0, |vr| -> Result<regex::Regex, Box<dyn std::error::Error + Send + Sync>> {
            Ok(regex::RegexBuilder::new(vr.as_str()?).case_insensitive(case_insensitive).build()?)
        })?;
        let text = ctx.get_raw(1).as_str().unwrap_or("");
        Ok(re.is_match(text))
    }).unwrap();
    c
}

fn flag(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

/// A time a person or agent types, as the UTC text the store keeps (2026-11-18T22:00:00.000Z).
fn when(v: &str) -> String {
    let c = Connection::open_in_memory().unwrap();
    let fmt = "%Y-%m-%dT%H:%M:%fZ";
    let (num, unit) = v.split_at(v.len().saturating_sub(1));
    let words: Vec<&str> = v.split_whitespace().collect();
    let unit_secs = |u: &str| match u.trim_end_matches('s') { "m" | "min" | "minute" => Some(60), "h" | "hour" => Some(3600), "d" | "day" => Some(86400), _ => None };
    let ago = match (num.parse::<i64>(), unit) {
        (Ok(n), "m") => Some(n * 60), (Ok(n), "h") => Some(n * 3600), (Ok(n), "d") => Some(n * 86400),
        _ => match words.as_slice() {
            [n, u, "ago"] => n.parse::<i64>().ok().zip(unit_secs(u)).map(|(n, s)| n * s),
            ["now"] => Some(0),
            _ => None,
        },
    };
    let day = match v { "today" => Some("0 days"), "yesterday" => Some("-1 days"), "tomorrow" => Some("+1 days"), _ => None };
    let out: Option<String> = if let Some(shift) = day {
        c.query_row("select strftime(?1, date('now','localtime', ?2), 'utc')", params![fmt, shift], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    } else if let Some(secs) = ago {
        c.query_row("select strftime(?1, 'now', ?2)", params![fmt, format!("-{secs} seconds")], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    } else if v.len() <= 5 && v.contains(':') {
        c.query_row("select strftime(?1, date('now','localtime') || ' ' || ?2, 'utc')", params![fmt, v], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    } else if v.ends_with('Z') || v.get(10..).is_some_and(|t| t.contains('+') || t.rfind('-').is_some()) {
        c.query_row("select strftime(?1, ?2)", params![fmt, v], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    } else {
        c.query_row("select strftime(?1, ?2, 'utc')", params![fmt, v.replace('T', " ")], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    };
    out.unwrap_or_else(|| die(&format!("can't read the time '{v}': use 2h, '2 hours ago', today, yesterday, 15:30, 2026-11-18 15:30, or ISO")))
}

fn local(ts: &str) -> String {
    let c = Connection::open_in_memory().unwrap();
    c.query_row("select strftime('%m-%d %H:%M:%S', ?1, 'localtime')", [ts], |r| r.get(0)).unwrap_or_else(|_| ts.to_string())
}

fn one_line(t: &str, max: usize) -> String {
    let t = t.replace(['\n', '\r', '\t'], " ");
    if t.chars().count() <= max { return t }
    format!("{}…", t.chars().take(max).collect::<String>())
}

/// The part of `text` around its first regex match, so a long tool result shows the line that matched.
fn around(text: &str, re: &regex::Regex, width: usize) -> String {
    match re.find(text) {
        Some(m) => {
            let mut start = m.start().saturating_sub(width / 3);
            while !text.is_char_boundary(start) { start -= 1 }
            let head = if start > 0 { "…" } else { "" };
            format!("{head}{}", one_line(&text[start..], width))
        }
        None => one_line(text, width),
    }
}

struct Hit { id: i64, ts: String, agent: String, sid: String, turn: i64, role: String, tool: Option<String>, text: String }

fn print_hit(h: &Hit, mark: &str) {
    let who = if h.agent.is_empty() { "-" } else { &h.agent };
    let role = match &h.tool { Some(t) => format!("{}[{t}]", h.role), None => h.role.clone() };
    println!("{mark}{}  {who}  {}:{}  {role}  {}", local(&h.ts), &h.sid[..8.min(h.sid.len())], h.turn, h.text);
}

fn search(a: &[String]) {
    let valued = ["-C", "-m", "--agent", "--session", "--role", "--tool", "--cwd", "--since", "--until"];
    let switches = ["-E", "-i", "-l", "-c"];
    let mut words = Vec::new();
    let mut i = 0;
    while i < a.len() {
        if valued.contains(&a[i].as_str()) { i += 2 }
        else if switches.contains(&a[i].as_str()) { i += 1 }
        else if a[i].starts_with('-') && a[i].len() > 1 && !a[i].starts_with("--") && a[i].chars().nth(1).is_some_and(|c| c.is_ascii_alphabetic()) {
            die(&format!("unknown option {}; see ctxgrep --help", a[i]))
        } else { words.push(a[i].clone()); i += 1 }
    }
    let regex = a.iter().any(|x| x == "-E").then(|| words.join(" "));
    let count = a.iter().any(|x| x == "-c");
    let ci = a.iter().any(|x| x == "-i");
    let list = a.iter().any(|x| x == "-l");
    let ctx: i64 = flag(a, "-C").map(|x| x.parse().unwrap_or_else(|_| die("-C takes a number"))).unwrap_or(0);
    let max: i64 = flag(a, "-m").map(|x| x.parse().unwrap_or_else(|_| die("-m takes a number"))).unwrap_or(20);
    if words.is_empty() { die("give words to search for (or a regex with -E); see ctxgrep --help") }
    let re = regex.as_ref().map(|p| regex::RegexBuilder::new(p).case_insensitive(ci).build().unwrap_or_else(|e| die(&format!("bad regex: {e}"))));

    let c = ro(ci);
    let (from, matcher) = match &regex {
        Some(_) => ("spans s join sessions x using(session_id)", "regexp(?1, s.text)"),
        None => ("spans_fts join spans s on s.id = spans_fts.rowid join sessions x using(session_id)", "spans_fts match ?1"),
    };
    let filters = "(?2 is null or x.agent = ?2) and (?3 is null or s.session_id like ?3 || '%') and (?4 is null or s.role = ?4)
        and (?5 is null or s.tool = ?5) and (?6 is null or x.cwd like '%' || ?6 || '%')
        and (?7 is null or s.ts >= ?7) and (?8 is null or s.ts < ?8)";
    let shown = if regex.is_some() { "s.text" } else { "snippet(spans_fts, 0, '[', ']', '…', 30)" };
    // A bare word with punctuation (install-walk, foo.rs, a/b) is an FTS5 syntax error or a column filter, so quote it as a phrase.
    // Words already in FTS syntax (quoted, OR/AND/NOT/NEAR, a trailing *) pass through unchanged.
    let fts_word = |w: &String| if w.contains('"') || w.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '*') { w.clone() } else { format!("\"{w}\"") };
    let pattern = regex.clone().unwrap_or_else(|| words.iter().map(fts_word).collect::<Vec<_>>().join(" "));
    let ps = params![pattern, flag(a, "--agent"), flag(a, "--session"), flag(a, "--role"), flag(a, "--tool"), flag(a, "--cwd"),
        flag(a, "--since").map(|v| when(&v)), flag(a, "--until").map(|v| when(&v)), max];

    if count {
        let q = format!("select count(*) from {from} where {matcher} and {filters} and ?9 is not null");
        let n: i64 = c.query_row(&q, ps, |r| r.get(0)).unwrap_or_else(|e| die(&e.to_string()));
        println!("{n}");
        if n == 0 { explain(&c, from, matcher, &pattern) }
        std::process::exit(if n > 0 { 0 } else { 1 });
    }
    if list {
        let q = format!("select s.session_id, coalesce(x.agent,''), min(s.ts), max(s.ts), count(*) from {from}
            where {matcher} and {filters} group by s.session_id order by max(s.ts) desc limit ?9");
        let mut st = c.prepare(&q).unwrap_or_else(|e| die(&e.to_string()));
        let mut n = 0;
        let mut rows = st.query(ps).unwrap_or_else(|e| die(&e.to_string()));
        while let Some(r) = rows.next().unwrap_or_else(|e| die(&e.to_string())) {
            let (sid, agent, first, last, hits): (String, String, String, String, i64) = (r.get(0).unwrap(), r.get(1).unwrap(), r.get(2).unwrap(), r.get(3).unwrap(), r.get(4).unwrap());
            println!("{sid}  {}  {} to {}  {hits} hits", if agent.is_empty() { "-" } else { &agent }, local(&first), local(&last));
            n += 1;
        }
        if n == 0 { explain(&c, from, matcher, &pattern) }
        std::process::exit(if n > 0 { 0 } else { 1 });
    }

    let q = format!("select s.id, s.ts, coalesce(x.agent,''), s.session_id, s.turn, s.role, s.tool, {shown} from {from}
        where {matcher} and {filters} order by s.ts desc, s.id desc limit ?9");
    let mut st = c.prepare(&q).unwrap_or_else(|e| die(&e.to_string()));
    let hits: Vec<Hit> = st.query_map(ps, |r| Ok(Hit {
        id: r.get(0)?, ts: r.get::<_, Option<String>>(1)?.unwrap_or_default(), agent: r.get(2)?, sid: r.get(3)?, turn: r.get(4)?,
        role: r.get(5)?, tool: r.get(6)?, text: r.get(7)?,
    })).unwrap_or_else(|e| die(&e.to_string())).filter_map(Result::ok).collect();
    if hits.is_empty() { explain(&c, from, matcher, &pattern); std::process::exit(1) }
    for (k, h) in hits.iter().enumerate() {
        let text = match &re { Some(re) => around(&h.text, re, 220), None => one_line(&h.text, 220) };
        if ctx == 0 { print_hit(&Hit { text, ..clone_hit(h) }, ""); continue }
        if k > 0 { println!("--") }
        let mut st = c.prepare("select s.id, s.ts, coalesce(x.agent,''), s.session_id, s.turn, s.role, s.tool, s.text
            from spans s join sessions x using(session_id) where s.session_id = ?1 and s.turn between ?2 and ?3 order by s.turn, s.id").unwrap();
        let near: Vec<Hit> = st.query_map(params![h.sid, h.turn - ctx, h.turn + ctx], |r| Ok(Hit {
            id: r.get(0)?, ts: r.get::<_, Option<String>>(1)?.unwrap_or_default(), agent: r.get(2)?, sid: r.get(3)?, turn: r.get(4)?,
            role: r.get(5)?, tool: r.get(6)?, text: r.get(7)?,
        })).unwrap().filter_map(Result::ok).collect();
        for n in near {
            let is_hit = n.id == h.id;
            let t = if is_hit { text.clone() } else { one_line(&n.text, 220) };
            print_hit(&Hit { text: t, ..n }, if is_hit { "> " } else { "  " });
        }
    }
}

/// Nothing matched: say on stderr whether the words or the filters ruled everything out.
fn explain(c: &Connection, from: &str, matcher: &str, pattern: &str) {
    let q = format!("select count(*) from {from} where {matcher}");
    match c.query_row(&q, [pattern], |r| r.get::<_, i64>(0)) {
        Ok(0) => eprintln!("ctxgrep: nothing in any session matches '{pattern}', before any filter"),
        Ok(n) => eprintln!("ctxgrep: '{pattern}' matches {n} turns in all; the filters ruled them all out"),
        Err(e) => eprintln!("ctxgrep: {e}"),
    }
}

fn clone_hit(h: &Hit) -> Hit {
    Hit { id: h.id, ts: h.ts.clone(), agent: h.agent.clone(), sid: h.sid.clone(), turn: h.turn, role: h.role.clone(), tool: h.tool.clone(), text: h.text.clone() }
}

fn session(a: &[String]) {
    let Some(who) = a.first() else { die("give a session id or an agent name") };
    let c = ro(false);
    let sid: String = c.query_row(
        "select session_id from sessions where session_id like ?1 || '%' or agent = ?1 order by ended_at desc limit 1",
        [who], |r| r.get(0)).optional().unwrap().unwrap_or_else(|| die(&format!("no session or agent '{who}'")));
    let from: i64 = flag(a, "--from").and_then(|x| x.parse().ok()).unwrap_or(0);
    let to: i64 = flag(a, "--to").and_then(|x| x.parse().ok()).unwrap_or(i64::MAX);
    let talk = a.iter().any(|x| x == "--talk");
    eprintln!("session {sid}");
    let mut st = c.prepare("select s.id, s.ts, coalesce(x.agent,''), s.session_id, s.turn, s.role, s.tool, s.text
        from spans s join sessions x using(session_id)
        where s.session_id = ?1 and s.turn between ?2 and ?3 and (?4 = 0 or s.role in ('user','assistant')) order by s.turn, s.id").unwrap();
    let rows: Vec<Hit> = st.query_map(params![sid, from, to, talk as i64], |r| Ok(Hit {
        id: r.get(0)?, ts: r.get::<_, Option<String>>(1)?.unwrap_or_default(), agent: r.get(2)?, sid: r.get(3)?, turn: r.get(4)?,
        role: r.get(5)?, tool: r.get(6)?, text: r.get(7)?,
    })).unwrap().filter_map(Result::ok).collect();
    for h in rows { let t = one_line(&h.text, 400); print_hit(&Hit { text: t, ..h }, "") }
}

/// This process's Claude Code session: walk up the parents to the one with a ~/.claude/sessions/<pid>.json.
fn own_session() -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..12 {
        if let Ok(t) = std::fs::read_to_string(home().join(format!(".claude/sessions/{pid}.json"))) {
            return serde_json::from_str::<Value>(&t).ok().and_then(|v| s(&v, "sessionId"));
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        pid = stat.rsplit(')').next()?.split_whitespace().nth(1)?.parse().ok()?;
        if pid <= 1 { return None }
    }
    None
}

/// The session before this one, for the same agent: what a fresh context reads first.
fn last(a: &[String]) {
    let c = ro(false);
    let me = own_session();
    let agent: Option<String> = a.iter().find(|x| !x.starts_with('-')).cloned().or_else(|| me.as_ref().and_then(|m|
        c.query_row("select agent from sessions where session_id = ?", [m], |r| r.get(0)).optional().ok().flatten().flatten()));
    let Some(agent) = agent else { die("no agent name for this session yet; give one: ctxgrep last <agent>") };
    let sid: String = c.query_row(
        "select session_id from sessions where agent = ?1 and session_id != coalesce(?2, '') order by ended_at desc limit 1",
        params![agent, me], |r| r.get(0)).optional().unwrap().unwrap_or_else(|| die(&format!("no earlier session for {agent}")));
    let mut args = vec![sid, "--talk".to_string()];
    args.extend(a.iter().filter(|x| x.starts_with('-') || x.parse::<i64>().is_ok()).cloned());
    session(&args)
}

fn sql(a: &[String]) {
    let q = a.join(" ");
    if q.trim().is_empty() { die("give a query: ctxgrep sql \"select ...\"") }
    let c = ro(false);
    let mut st = c.prepare(&q).unwrap_or_else(|e| die(&format!("{e}")));
    let cols: Vec<String> = st.column_names().iter().map(|x| x.to_string()).collect();
    println!("{}", cols.join("\t"));
    let mut rows = st.query([]).unwrap_or_else(|e| die(&format!("{e}")));
    while let Some(r) = rows.next().unwrap_or_else(|e| die(&format!("{e}"))) {
        let vals: Vec<String> = (0..cols.len()).map(|i| match r.get_ref(i).unwrap() {
            rusqlite::types::ValueRef::Null => String::new(),
            rusqlite::types::ValueRef::Integer(x) => x.to_string(),
            rusqlite::types::ValueRef::Real(x) => x.to_string(),
            rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).replace(['\n', '\t'], " "),
            rusqlite::types::ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
        }).collect();
        println!("{}", vals.join("\t"));
    }
}
