# aika-server

Server emulator for the MMORPG **Aika Online**, written in Rust.

Ported from two references: the original **Delphi** server (authoritative — it is
what our client actually talks to) and **[AikaEmu](https://github.com/lemestwo/AikaEmu)**
in C# (GPL-3.0, more readable, used for cross-checking). Because of AikaEmu this
project is GPL-3.0 too.

## Never in a tracked file

- **No absolute paths.** A path carries the username and the folder layout of
  the machine it came from. Data outside the repository is referenced through a
  relative path in `config.toml`, and the data itself goes in `data/`, which is
  ignored. This rule exists because it was broken once and pushed.
- **No real account data.** The original MySQL dump had live emails and password
  hashes; only `sql/schema.sql` (structure, no rows) is tracked.
- **No client or original-server data.** `SL.bin`, `ItemList.bin`, `data/NPCs`
  and anything else from the pack is read locally and never redistributed.

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
- **Data files are Delphi records written straight to disk.** `.npc` is a raw
  `TNPCFile`. Read the record definition, then confirm each offset against all
  the files at once by searching for the value that has to be there.
- **The `.npc` id is in the file name, not in the record.** The files were made
  by copying one another; `[2700] Lilola Hawn.npc` says 2215 inside. The
  original patches a hardcoded few and lets the rest overwrite each other.
- **Players and NPCs share one id space.** 1..2000 for players, 2048..3048 for
  NPCs (`Connections/ServerSocket.pas:44`). Player ids are capped so a
  connection can never be drawn on top of a townsperson.

## Database

SQLite during development, **MySQL in production**. The persistence layer stays
portable: every query lives in `crates/aika-server/src/db.rs`, only
`INTEGER`/`TEXT`/`BLOB`/`REAL`, timestamps as integer unix seconds, no
`INSERT OR REPLACE` (SQLite-only) and no `REPLACE INTO` (MySQL-only).

The database is the truth. `config.toml` seeds an *empty* one so a fresh
checkout has somewhere to log in, and is ignored from then on: to seed again,
delete `aika.db`. Positions are written as a player disconnects, so logging out
somewhere means logging back in there; `tests/persistence.rs` proves it across
two servers that share nothing but the file.

## Running

```sh
cargo run -p aika-server -- config.toml   # from the repo root
RUST_LOG=aika_server=debug cargo run -p aika-server -- config.toml
```

Channels live in `config.toml`; accounts and characters live in `aika.db` after
the first run. No rebuild is needed after editing either.
