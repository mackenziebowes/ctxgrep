# ctxgrep

`ctxgrep` searches every Claude Code conversation you have ever had, on this machine, in one command, so you can find "that thing we figured out last week" without remembering which project it was in.

It reads the transcripts Claude Code already keeps in `~/.claude/projects` (your messages, Claude's replies, and the output of every tool it ran) and builds a small search index from them. Your transcripts are never changed, and nothing leaves your machine.

## Example

You remember fixing a checkout bug that had something to do with `juniper`, but not where:

```
$ ctxgrep juniper
11-19 20:41:30  -  7be40d15:2  assistant  Astro. [Juniper] needs a Python runtime on the host, and the blog deploys to plain static hosting.
11-19 20:41:09  -  7be40d15:1  user  which static site generator did we pick for the blog, and why not [juniper]?
11-10 15:03:05  -  3f9c1a72:4  assistant  …[juniper] 4.2 renamed the cookie from [juniper]-session to jn_sess, so checkout.py:41 raises KeyError. I'll read the new name from the config instead of hard…
11-10 15:02:41  -  3f9c1a72:3  tool_result[Bash]  src/checkout.py:41:    token = request.cookies["[juniper]-session"] KeyError: '[juniper]-session'
11-10 15:02:11  -  3f9c1a72:1  user  checkout is returning a 500 since I upgraded [juniper] to 4.2, can you look?
```

Newest first, one hit per line: local time, agent name (the session's name, if Claude Code recorded one as `agentName` in its transcript; `-` if not), `session:turn`, who said it (`user`, `assistant`, or `tool_result[Bash]` for a tool's output), then the text with your match in `[brackets]`.

Want the surrounding conversation? Add `-C 1` for one turn either side, and narrow by project with `--cwd`:

```
$ ctxgrep -C 1 --cwd shop KeyError
  11-10 15:02:41  -  3f9c1a72:3  tool_result[Bash]  src/checkout.py:41:    token = request.cookies["juniper-session"] KeyError: 'juniper-session'
> 11-10 15:03:05  -  3f9c1a72:4  assistant  …juniper 4.2 renamed the cookie from juniper-session to jn_sess, so checkout.py:41 raises [KeyError]. I'll read the new name from the config instead of hard…
```

Then read a whole session by the id from the second column (a prefix is enough):

```
$ ctxgrep session 7be40d15
session 7be40d15-0000-4000-8000-000000000002
11-19 20:41:09  -  7be40d15:1  user  which static site generator did we pick for the blog, and why not juniper?
11-19 20:41:30  -  7be40d15:2  assistant  Astro. Juniper needs a Python runtime on the host, and the blog deploys to plain static hosting.
```

(The sessions above are made up for this page; yours will be your own.)

## Commands

| Command | What it does |
|---|---|
| `ctxgrep index` | Add anything new from your transcripts to the index. Quick after the first run. **Run it before you search**; searching does not update the index by itself. |
| `ctxgrep <words>` | Search. `juniper desk` needs both words; also `"exact phrase"`, `juni*`, `a OR b`, `a NOT b`. |
| `ctxgrep -E '<regex>'` | Search by regular expression instead of words (slower on a big history; combine with `--since` or `--cwd`). |
| `ctxgrep -C 2 <words>` | Show 2 turns before and after each hit. |
| `ctxgrep -l <words>` / `-c <words>` | List the matching sessions / count the matching turns. |
| `ctxgrep --since 3d --role user <words>` | Filter by time (`30m`, `2h`, `3d`, `yesterday`, `2026-11-18 15:30`), `--role` (`user`, `assistant`, `tool_use`, `tool_result`), `--tool Bash`, `--cwd <folder text>`, `--agent <name>`. |
| `ctxgrep session <id>` | Read one session in order (`--talk` for just the conversation, `--from N --to M` for a range). |
| `ctxgrep sql "select ..."` | Read-only SQL over the index (tables `sessions` and `spans`). |
| `ctxgrep rebuild` | Throw the index away and rebuild it from the transcripts. Safe: the transcripts are the source of truth. |
| `ctxgrep doctor` | Check the install: one line per check, `ok` or what to fix. Exits 1 if anything fails. |

`ctxgrep --help` has the full list. Exit code: 0 if something matched, 1 if nothing did, 2 on an error.

## Install

Ask your Claude Code: "Install ctxgrep from https://github.com/mackenziebowes/ctxgrep by following its INSTALL.md." It builds `ctxgrep` from source (needs Rust and a C compiler), installs it into a folder you pick, and ends by running `ctxgrep doctor`.

## What is not in this release

This folder also holds things that only work on Mackenzie's machine. They are not part of `ctxgrep` and the install does not use them:

- `hooks/ctxgauge`: Claude Code hooks that type `/clear` into a tmux pane and hand over to the next session. They need Mackenzie's tmux setup and agent tooling.
- `backup/`: a systemd timer that copies transcripts with rsync to a disk at `/data2`.
- `index/`: systemd units that run `ctxgrep index` every minute from `~/.cargo/bin`. Until a portable version exists, run `ctxgrep index` yourself before searching.

## Limits

- The first `ctxgrep index` reads your whole history. It took longer than ten minutes on Mackenzie's 27 GB of transcripts and was still going, so a large history needs patience. Later runs read only what is new.
- The index holds a copy of your conversations' text, so treat the index file (`~/.local/share/ctxgrep/ctxgrep.db`) as private, like the transcripts themselves.
- Each piece of text is cut at 100,000 bytes in the index. Claude's private "thinking" is not indexed.
- Built and tested on Linux. macOS is expected to work but has not been tried. `ctxgrep last` (previous session of a named agent) needs Linux and a named session.

MIT licensed.
