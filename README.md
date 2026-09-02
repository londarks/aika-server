# aika-server

Server emulator for the MMORPG **Aika Online**, written in Rust.

Ported from the original **Delphi** server, which is the authority throughout:
it is what our client actually talks to. Every behaviour here is traced back to
the file in that source which owns it, and the commit that adds it says which.

> No game client, asset or binary from Aika Online is included here. This
> repository is server source code only.

## Status

| Layer | State |
|---|---|
| Packet cipher | done — the 512-key table and the alternation are `Connections/EncDec.pas`, byte for byte |
| TCP framing | done (reassembles fragments, splits coalesced packets, drops the client prefix) |
| Token server (HTTP) | done — issues and validates tokens, character count, channel status, launcher routes |
| Login server (TCP) | done — validates the token and clears the account |
| Characters | list, creation from the class templates, deletion, world entry |
| Movement and chat | walking, turning, say and whisper, emotes, sitting and dancing |
| Combat | swings, spells, damage with both animations, dying and getting up |
| Monsters | the two clocks of `Mob/MOB.pas`: patrol, aggro, chase, leash, respawn, drops |
| Skills | the class grid, learning at a trainer, casting, cooldowns per family |
| Items | shops (gold, honor, medals, item currency), equipment, stacking, splitting, durability |
| Storage | the account chest: 86 slots, four pages, gold in and out |
| Buffs | potions and saddles start them; they expire on their own |
| Mounts | worn, drawn on the rider, and their own two skills |
| Persistence | SQLite in development, MySQL in production — position, gold, items, skills, chest |
| Game data | `ItemList.bin`, `SkillData.bin`, `ExpList.bin`, `.npc`, mob CSVs, drops, `SL.bin` |

405 tests, including end-to-end runs over real sockets: HTTP token, TCP login
and game server through to the character list, and a second server started on
the same database file to prove what was saved.

**The original 2008 client logs in, fights, trades, learns skills and rides.**

## Roadmap

What is left, in the order it is worth doing. The ordering is not by size: it
is by how much of the game each one unlocks for somebody playing alone, since
that is how this is tested.

**1. Effects.** The one piece everything else is waiting on. A skill and an
item each carry an `EF`/`EFV` pair of arrays, and `GetCurrentScore` reads them
through `GetMobAbility` to arrive at the numbers a character really has. Until
it lands, a buff starts and shows and does nothing: a potion promising ten per
cent more of everything changes no number, a mount adds no speed, and rings and
necklaces are worth zero because that is where their whole value lives.

**2. The rest of the character sheet.** Spending status points (`0x213`), the
skill reset, and making a learned rank change what a spell does — learning
raises the rank in the record today, but the damage is still the first rank's.

**3. Keeping and mending things.** Repair, enchant, craft. The item types are
already identified in `UseItem`; the work is the tables behind them.

**4. The small ones.** Quests, dungeons.

**5. Channels.** Changing channel needs the world split per channel first —
today all four share one — and then the token handshake of `LoginIntoChannel`.
Large, and worth little until there is somebody else online.

**6. Everything that needs other people.** Guilds, parties and raids, friends
and duels, trading, nations and relics, mail, the auction house, titles and
events. About half the remaining opcodes, and none of it testable alone.

Two things are already settled and should not be looked for again: the numeric
PIN and `KarakAereo` are both dead code in the original, sitting behind an
`Exit;` at the top of their handlers, and the live half of each is already
implemented here.

## The database

SQLite while developing, MySQL in production, and the same code for both.
A fresh checkout needs no setup: `config.toml` points at a file under
`var/`, and the file is created on first run.

```toml
[database]
path = "var/aika.db"      # the development database
url  = ""                 # a full connection string, when there is one
```

For MySQL, set the connection string **in the environment**, not in the
file:

```sh
export AIKA_DATABASE_URL='mysql://user:password@host:3306/database'
cargo run -p aika-server -- config.toml
```

`AIKA_DATABASE_URL` wins over `url`, which wins over `path`. The schema is
created on connection either way, so an empty database is enough to start.

### Why the URL is not in `config.toml`

It carries a password and `config.toml` is tracked. This is the same rule
as the one about absolute paths, and it is written down in both places for
the same reason: it was broken once and pushed. Anything that identifies a
machine, an account or a person stays out of the repository.

The one connection string that is written down is a redaction: the server
logs `mysql://user:***@host/db` and its errors say the same, because the
first thing anybody does with a connection error is paste it somewhere.

### Keeping one schema for two databases

Only `INTEGER`, `TEXT`, `BLOB` and `REAL`. Timestamps are integer seconds.
No `INSERT OR REPLACE`, which is SQLite's, and no `REPLACE INTO`, which is
MySQL's — an upsert is an `UPDATE` and then an `INSERT` if it changed
nothing.

Three differences needed real handling rather than a rule:

- **The self-counting key.** `AUTOINCREMENT` is SQLite's spelling and
  `AUTO_INCREMENT` on an `INT` is MySQL's. The schema is written once and
  the key is rewritten on the way out.
- **The id of a row just inserted.** `RETURNING id` is SQLite's and
  Postgres's; MySQL has never had it. `LAST_INSERT_ID()` is per connection
  and a pool hands the next query to whichever connection is free, which is
  right almost always and wrong under load. So the row is read back by
  whatever made it unique.
- **A database in memory belongs to one connection.** `sqlite::memory:`
  with a pool of eight is eight empty databases and a schema in one of
  them, so that one case takes a single connection.

## Running

```sh
cargo run -p aika-server -- config.toml     # from the repository root
RUST_LOG=aika_server=debug cargo run -p aika-server -- config.toml
```

All three services come up in one process, the way the original server does:
token on 8090, login on 8831 and the game server on 8822. Accounts, characters
and channels live in `config.toml`; editing it needs no rebuild.

To exercise it without the client:

```sh
TOKEN=$(curl -s -X POST -d "id=admin&pw=admin" http://127.0.0.1:8090/member/aika_get_token.asp)
curl -s -X POST -d "id=admin&pw=$TOKEN" http://127.0.0.1:8090/servers/aika_get_chrcnt.asp
# => CNT 1 0 0 0<br>2 0 0 0    (1 character, nation 2)
```

The integration test (`cargo test -p aika-server`) walks the same path the
client does, both binary sockets included, which is how the server is verified
without depending on anything external.

## How the client gets in

1. **HTTP** `POST /member/aika_get_token.asp` with `id` and `pw` returns a
   32 hex token, valid for five minutes. Errors are numbers: `0` no such
   account, `-1` wrong password, `-8` banned.
2. **HTTP** `POST /servers/aika_get_chrcnt.asp` with `id` and the token returns
   `CNT n 0 0 0<br>nation 0 0 0`.
3. **HTTP** `POST /servers/serv00.asp` returns the population of each channel,
   space separated (`-1` means offline). It must carry **one value per
   channel** — pad it and the channel list comes up empty.
4. **TCP 8831** — packet `0x81` with username and token; on a match the server
   answers `0x82` with the account id and nation, then hangs up.
5. **TCP 8822** — packet `0x685` with the account and the client version, which
   must be exactly **124**. The server answers `0x901` (336 bytes) with the
   three character slots: the selection screen. Anything refused drops the
   connection, which is what makes the client show an error instead of waiting.

   The Delphi record declares 1096 bytes for this packet, but **the client
   sends 100** — the trailing 992 bytes of the record are receive buffer and
   never reach the wire. Distrust any record ending in a large padding array.
6. **TCP 8822** — `0xF02` picks the character; the server answers `0xCCCC`,
   three `0x186` and `0x925` (6400 bytes) carrying the whole character.
7. **TCP 8822** — the client reports `0xF0B` ("loaded") and the server sends
   `0x349` (508 bytes), which **places** the character on the map. Without that
   last one the player enters the world and floats in the middle of nowhere:
   `0x925` says who they are, not where.

   Coordinates are always a pair of `f32`, and **there is no Z** — height comes
   from the terrain the client already has.

   `0x349` may only be sent **once per session**. The client resends `0xF0B`
   whenever it thinks something is missing, including when trying to walk;
   spawning again teleports the player back to the start and traps the intro in
   a loop.

The first packet on each binary socket arrives with 4 junk bytes in front. The
original server cuts them blindly; here the call is made from the content, so
the server also works with a client that sends no prefix.

### Channels are separated by IP, not by port

The Delphi server hardcodes port 8822 for every channel; what tells channels
apart is the **IP**. The client's `SL.bin` points its channels at `127.0.0.1`
through `127.0.0.4`, so the server opens one socket per address. Listening on
only one of them gives `connect error: 10061` on every other channel.

The client also shows **only the channel whose `NationIndex` matches the
account's nation**.

### Quirks the client depends on

They look like bugs and are not:

- **Level travels as one less.** The client adds 1, so a level 42 character
  goes out as 41. True in the character list and in `TCharacter`.
- **`Refine[7]` is always 15**, hardcoded, with the weapon's real refine
  commented out in the original.
- **`Equip[0]` and `Equip[1]` are not equipment**: they are the class index and
  the hair index. The rest of the array is appearance.
- **The `Index` header field of `0x925` is a fixed `0x7535`**, not the client id.
- Offset comments inside the Delphi records are stale and disagree with each
  other by 16 and 32 bytes. Trust the declared types.

## Packet format

Little-endian throughout:

```
0..2   u16  total frame size
2      u8   checksum
3      u8   cipher seed (drawn per packet)
4..6   u16  index / sender
6..8   u16  opcode
8..12  u32  timestamp
12..   body
```

The cipher works on 4-byte words from offset 4 onward; the seed picks a start
position in a fixed table of 256 keys, which advances one per word, and the
operation alternates between `+4k`, `-(k>>1)`, `+2k` and `-(k>>2)`.

**The checksum does not protect the payload.** The cipher is linear, so the
difference between the sums depends only on the seed and the length: flipping a
body byte still validates. That is how the original game behaves. It is not an
integrity check and must not be treated as one.

## Crates

- **`aika-net`** — cipher (`crypto.rs`) and framing (`frame.rs`). No I/O: feed
  it bytes in whatever order they arrive and it returns complete messages,
  which makes it testable without a socket.
- **`aika-server`** — the three services. The HTTP layer is hand-written on
  purpose: the client is from 2008 and sends requests modern servers reject
  with 400 (no `Host`, HTTP/1.0, bare `\n` line endings), and a 400 here turns
  into a login screen that hangs with no explanation.
- **`aika-data`** — game file formats. `SL.bin` so far, with the `sl-tool`
  binary:

  ```sh
  cargo run -p aika-data --bin sl-tool -- list /path/to/client/SL.bin
  cargo run -p aika-data --bin sl-tool -- set-ip /path/to/client/SL.bin 192.168.0.10
  ```

  Each channel keeps its original 72 bytes, so editing the IP touches no other
  byte, not even the fields nobody understands. A file read and written back
  without edits comes out identical to the original.

  Client files are not redistributed here. Drop an `SL.bin` from a client into
  `crates/aika-data/testdata/` and the round-trip tests start checking against
  real bytes; without it they run against a list the codec builds itself.

  It also reads `strdef*.bin`, the client string table, through `strdef-tool`:

  ```sh
  strdef-tool list    /path/to/client/UI/strdef4.bin
  strdef-tool pending /path/to/client/UI/strdef4.bin   # entries still in Big5 or EUC-KR
  strdef-tool export  /path/to/client/UI/strdef4.bin pt.tsv
  strdef-tool import  /path/to/client/UI/strdef4.bin pt.tsv
  strdef-tool scan    /path/to/client/UI/FieldSceneB4.bin
  ```

  And `jit-tool` converts the client textures, which is what the whole
  interface is made of:

  ```sh
  jit-tool info   /path/to/client/UI/1024login.jit
  jit-tool to-dds /path/to/client/UI/1024login.jit    # edit the DDS anywhere
  jit-tool to-jit /path/to/client/UI/1024login.jit edited.dds
  ```

  A `.jit` is a twelve-byte header in front of raw DXT blocks, so the pixel
  data is copied untouched in both directions and a round trip changes nothing.
  `to-jit` takes the original as a template so it can refuse a replacement the
  client would not accept, and writes to a `.new` file.

  The table is 3096 fixed 128-byte records of plain latin-1 text, so the whole
  interface can be retranslated by editing a TSV. Records nobody touched come
  back byte for byte, and `import` writes to a `.new` file rather than over the
  original. `scan` handles the files that are not record tables, walking the
  bytes for embedded untranslated strings.

## `sql/schema.sql`

The structure of the original server's database, 47 tables, kept as
documentation of which fields the game needs. It is **not** the schema this
project uses — we design our own from it. Every `INSERT` was stripped: the
original dump came from a private server that really ran and carried real
account rows, e-mail addresses and password hashes.

## License

See [LICENSE.md](LICENSE.md).
