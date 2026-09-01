# aika-server

Server emulator for the MMORPG **Aika Online**, written in Rust.

Ported from the original **Delphi** server, which is the authority throughout: it
is what our client actually talks to.

## Never in a tracked file

- **No absolute paths.** A path carries the username and the folder layout of
  the machine it came from. Data outside the repository is referenced through a
  relative path in `config.toml`, and the data itself goes in `data/`, which is
  ignored. This rule exists because it was broken once and pushed.
- **No real account data.** The original MySQL dump had live emails and password
  hashes; only `sql/schema.sql` (structure, no rows) is tracked.
- **No client or original-server data.** `SL.bin`, `ItemList.bin`, `assets/`
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

## Where things live

```text
crates/          the source; aika-net (protocol), aika-data (file formats),
                 aika-server (the server itself)
sql/             the original MySQL schema, structure only, as documentation
assets/          data copied from the original pack, read and never written:
                 assets/npcs/ and assets/items/. Ignored by git.
var/             everything the server writes: var/aika.db. Ignored by git.
config.toml      the one file to edit to run it
```

Nothing the server reads or writes sits at the root, and neither directory is
tracked: `assets/` is somebody else's data and `var/` is this machine's state.

## Layout

- `crates/aika-net` — packet cipher and TCP framing. No I/O: feed it bytes, get
  messages back, which makes it testable without sockets.
- `crates/aika-server` — the three services in one process, like the original:
  token HTTP (8090), login TCP (8831), game TCP (8822).
- `crates/aika-data` — game file formats (`SL.bin` so far) plus the `sl-tool`
  binary.
- `sql/` — the original MySQL dump. **Documentation of which fields the game
  needs, not a schema to copy.**

The reference source lives outside this repo, in a sibling directory:
`../aika-delphi-bin/Src`.

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
- **The Delphi server is the authority, and the only one.** It is what our
  client actually speaks to. Other emulators exist and describe a different
  build of the game with different tables; nothing here comes from one, and
  nothing should.
- **The packet checksum does not protect the payload.** The cipher is linear, so
  the sum difference depends only on seed and length. Not an integrity check.
- **Coordinates are always `f32` pairs, and there is no Z.** Height comes from
  the client's own terrain.
- **Keep the protocol at the edge.** Wire quirks (level minus one, fixed refine
  slots) are converted when encoding, never stored in game logic.
- **Find the file that owns the behaviour before writing any of it.** The
  monster AI lives in `Mob/MOB.pas`, which went unread for two attempts while
  behaviour was invented from fragments elsewhere. Every number was wrong:
  aggro 20 against 8, reach 12 against 3, a step of 22 against 1.5. Grep for
  the *state* a behaviour keeps (`MovedTo`, `IsAttacked`) rather than for the
  verb, and the owning file falls out.
- **Data files are Delphi records written straight to disk.** `.npc` is a raw
  `TNPCFile`. Read the record definition, then confirm each offset against all
  the files at once by searching for the value that has to be there.
- **The `.npc` id is in the file name, not in the record.** The files were made
  by copying one another; `[2700] Lilola Hawn.npc` says 2215 inside. The
  original patches a hardcoded few and lets the rest overwrite each other.
- **The client and the server have different item tables.** The server's
  `ItemList.bin` is plaintext; the client's `ItemList4.bin` is the same size
  plus a 12-byte `BR00022I` header and is encrypted with a position-dependent
  cipher (identical records encrypt differently, so no repeating-key attack).
  Shops reference ids the client has and the server does not — that is a data
  mismatch in the pack, not a bug. Reading the client's table would let the
  two be regenerated from one source; nobody has done it yet.
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
delete `var/aika.db`. Positions are written as a player disconnects, so logging out
somewhere means logging back in there; `tests/persistence.rs` proves it across
two servers that share nothing but the file.

## Running

```sh
cargo run -p aika-server -- config.toml   # from the repo root
RUST_LOG=aika_server=debug cargo run -p aika-server -- config.toml
```

Channels live in `config.toml`; accounts and characters live in `var/aika.db` after
the first run. No rebuild is needed after editing either.
