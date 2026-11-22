# Installing ctxgrep (for the agent doing the install)

You are installing `ctxgrep`, a command-line search over the user's Claude Code transcripts (`~/.claude/projects`). It builds from source with Rust. Follow the steps in order. Each has a **Check**; do not move on until it passes.

Do not touch `hooks/`, `backup/` or `index/` in this repo: they are for the author's machine only (see README.md, "What is not in this release").

If you don't have the repo yet, clone it: `git clone https://github.com/mackenziebowes/ctxgrep.git`; **Check:** the new `ctxgrep` folder holds `Cargo.toml`.

Throughout, `<REPO>` is the folder that contains this file (the one with `Cargo.toml`), and `<PREFIX>` is the folder the user wants the program installed under. **Ask the user for `<PREFIX>`** or propose one (for example `$HOME/.local/ctxgrep`). The program ends up at `<PREFIX>/bin/ctxgrep`. Use absolute paths.

## 1. Check the build tools

Run:

```
cargo --version
cc --version
```

**Check:** both print a version, for example `cargo 1.100.0 (...)` and `cc (GCC) 16.2.1 ...` (`cc` may say clang on macOS). This guide was run with that cargo.

**If not:**
- `cargo: command not found`: Rust is not installed. Tell the user, and with their agreement install it from <https://rustup.rs>, then open a new shell so `cargo` is on the PATH.
- `cc: command not found`: a C compiler is needed to build the bundled SQLite. Debian/Ubuntu: `sudo apt install build-essential`. macOS: `xcode-select --install`. Fedora: `sudo dnf install gcc`. Arch: `sudo pacman -S base-devel`.
- A later build error that mentions a Rust version: run `rustup update stable`.

## 2. Build and install

Run:

```
cargo install --path <REPO> --root <PREFIX> --locked
```

The first build downloads crates and compiles SQLite; about a minute.

**Check:**

```
<PREFIX>/bin/ctxgrep --version
```

prints `ctxgrep 0.1.0`.

**If not:**
- `could not find Cargo.toml`: `<REPO>` is wrong; it must be the folder holding `Cargo.toml`.
- `failed to download` / `Updating crates.io index` hangs: no network access to crates.io; fix the connection and re-run.
- `error: linker 'cc' not found` or a `cc` error: step 1's compiler is missing.
- Permission denied under `<PREFIX>`: pick a folder the user can write.

## 3. Make the command reachable (optional)

`cargo install` prints a warning that `<PREFIX>/bin` is not on the PATH. Either always call `<PREFIX>/bin/ctxgrep` by its full path, or add it to the PATH for this shell:

```
export PATH="<PREFIX>/bin:$PATH"
```

To make that permanent, tell the user to add that line to their shell profile (`~/.bashrc`, `~/.zshrc`); do not edit it for them without asking.

**Check:** `command -v ctxgrep` prints `<PREFIX>/bin/ctxgrep`.

**If not:** the export was not run in this shell, or a different `ctxgrep` is earlier on the PATH; use the full path.

The rest of this guide writes `ctxgrep` for whichever form works.

## 4. Index the transcripts

Run:

```
ctxgrep index
```

This reads `~/.claude/projects` (read-only) and writes the store at `~/.local/share/ctxgrep/ctxgrep.db`. To keep the store somewhere else, set `CTXGREP_DB=<path>` for this and every later command; likewise `CTXGREP_SRC=<folder>` if the transcripts are not in `~/.claude/projects`.

The first run reads the whole history. It was still running after ten minutes on a 27 GB history, so for a very large history run it in the background or with a long timeout, and tell the user it is working. Running it again later only reads what is new.

**Check:** one line like

```
indexed 6 spans from 2 changed files (2 files seen)
```

where the first number is above 0. Running it a second time prints `indexed 0 spans from 0 changed files (2 files seen)`.

**If not:**
- `indexed 0 spans from 0 changed files (0 files seen)`: no transcripts were found. The folder is wrong or empty; see the `source` line of `ctxgrep doctor` in step 5.
- A `ctxgrep: make folder ...` or `ctxgrep: open ...` error: the store's folder is not writable. Set `CTXGREP_DB` to a path the user can write.
- Interrupted partway: just run `ctxgrep index` again; it carries on where it stopped. If it ever misbehaves, `ctxgrep rebuild` drops the store and reads everything again.

## 5. Run the doctor

Run:

```
ctxgrep doctor
```

**Check:** exit code 0 and four lines, all starting with `ok`:

```
ok    binary: ctxgrep 0.1.0 runs; SQLite 3.50.2 with FTS5
ok    source: /home/you/.claude/projects has 42 transcript files
ok    store: /home/you/.local/share/ctxgrep/ctxgrep.db is writable
ok    index: 9034 spans indexed
```

(Your paths and numbers will differ.) A failing check starts with `FAIL` and says what to fix; the exit code is then 1.

**If not:**
- `FAIL  binary`: the build has no full-text search. Rebuild from a clean copy of `<REPO>` (step 2).
- `FAIL  source`: the transcript folder is missing or has no `.jsonl` files. Claude Code must have been used on this machine; if the transcripts live elsewhere, set `CTXGREP_SRC`.
- `FAIL  store`: the store's folder is not writable; set `CTXGREP_DB` to a path the user can write, then re-run step 4.
- `FAIL  index`: the store is empty; run step 4.

## 6. Search once

Ask the user for a word or phrase they remember from an old conversation, then run:

```
ctxgrep -m 5 <word>
```

**Check:** exit code 0 and up to five lines of the form `MM-DD HH:MM:SS  <agent>  <session>:<turn>  <role>  <text with [word] in brackets>`, for example:

```
11-19 20:41:30  -  7be40d15:2  assistant  Astro. [Juniper] needs a Python runtime on the host, ...
```

**If not:** exit code 1 and `ctxgrep: nothing in any session matches '<word>', before any filter` means the word is not in the history (or the index is old: run `ctxgrep index` and retry); try another word. Any other error: read the message; `ctxgrep --help` lists every option.

Done. Tell the user: run `ctxgrep index` before searching to pick up new conversations, and see README.md for the commands.
