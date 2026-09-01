# License

Copyright © the aika-server authors. All rights reserved.

No licence is granted yet. Read it, learn from it, open an issue — but there is
no permission here to copy, modify or redistribute it, because none has been
chosen. If you want to use any of this, ask.

## What this code is

Original work in Rust. What it takes from elsewhere is knowledge about a
network protocol — byte offsets, opcode numbers, packet sizes, the order things
are sent in — read out of the original Aika Online server written in Delphi,
which is the only thing that can settle what our client expects. Behaviour is
traced back to the file that owns it rather than guessed at, and the commit
that adds it says which file.

The cipher's 512-byte key table is that protocol's key material and cannot be
derived from anything else. It is `EncDecKeys` from `Connections/EncDec.pas`,
byte for byte.

## What this code is not

No game asset, client binary, item table, map or any other content from Aika
Online is included or distributed here. This repository is server source only.
The original server's database dump is kept as structure alone — every row was
stripped, because it came from a private server that really ran and carried
real e-mail addresses and password hashes.

Aika Online and everything in it belong to their respective rights holders.
This is a study of a protocol, written to make a client from 2008 work again.
