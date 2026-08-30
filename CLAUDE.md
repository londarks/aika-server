# aika-server

Server emulator for the MMORPG **Aika Online**, written in Rust.

Ported from two references: the original **Delphi** server (authoritative — it is
what our client actually talks to) and **[AikaEmu](https://github.com/lemestwo/AikaEmu)**
in C# (GPL-3.0, more readable, used for cross-checking). Because of AikaEmu this
project is GPL-3.0 too.

## Conventions

**Everything in English.** Code, comments, doc comments, log messages, test
names, assertion messages and commit messages. No mixed languages.

**Commits carry no AI attribution.** Do not add `Co-Authored-By: Claude`,
`Generated with Claude Code`, or any similar trailer or footer. Write commit
messages as the project author would: imperative mood, explaining *why* when it
is not obvious.

**Never execute binaries from the downloaded reference pack** (`AikaServer.exe`,
`BinParser.exe`, `PacketAnalyzer.exe`, `MasterEditor.exe`, installers). They are
documentation only: read the sources and data, reimplement in Rust. The one
agreed exception is the game client itself, used for visual testing.

## Layout

- `crates/aika-net` — packet cipher and TCP framing. No I/O: feed it bytes, get
  messages back, which makes it testable without sockets.
- `crates/aika-server` — the three services in one process, like the original:
  token HTTP (8090), login TCP (8831), game TCP (8822).
- `crates/aika-data` — game file formats (`SL.bin` so far) plus the `sl-tool`
  binary.
- `sql/` — the original MySQL dump. **Documentation of which fields the game
  needs, not a schema to copy.**

Reference sources live outside this repo, in sibling directories:
`../aika-delphi-bin/Src` (Delphi) and `../AikaEmu` (C#).

## Rules learned the hard way

- **Declared size is not wire size.** `TRequestLoginPacket` declares 1096 bytes;
  the client sends 100. Trailing padding arrays are receive buffers, not
  content. Distrust any record ending in a large padding array.
- **Three copies of the packet dispatch exist in the Delphi source.** Only
  `Threads/PlayerThread.pas:97` is live. The other two have no call sites.
- **Offset comments inside the Delphi records are stale** — they disagree with
  each other by 16 and 32 bytes. Trust the declared types, never the comments.
- **Dead code hides behind `Exit;`** — check that a handler's body is actually
  reachable before porting it.
- **When the two references disagree, the Delphi wins.** It is the one our
  client speaks to. (The AikaEmu README says the channel list has 56 values; the
  real client wants one per channel.)
- **The packet checksum does not protect the payload.** The cipher is linear, so
  the sum difference depends only on seed and length. Not an integrity check.
- **Coordinates are always `f32` pairs, and there is no Z.** Height comes from
  the client's own terrain.
- **Keep the protocol at the edge.** Wire quirks (level minus one, fixed refine
  slots) are converted when encoding, never stored in game logic.

## Database

SQLite during development, **MySQL in production**. The persistence layer must
stay portable from day one: all queries in one module, only `INTEGER`/`TEXT`/
`BLOB`/`REAL`, timestamps as integer unix seconds, no `INSERT OR REPLACE`
(SQLite-only) and no `REPLACE INTO` (MySQL-only).

## Running

```sh
cargo run -p aika-server -- config.toml   # from the repo root
RUST_LOG=aika_server=debug cargo run -p aika-server -- config.toml
```

Accounts, characters and channels live in `config.toml`; no rebuild needed after
editing it.
