# License

This project is licensed under the **GNU General Public License v3.0**.

It is a port of, and derives from, [AikaEmu](https://github.com/lemestwo/AikaEmu),
which is itself GPL-3.0. The full license text is available at
<https://www.gnu.org/licenses/gpl-3.0.txt>.

## Scope

The code in this repository is original work written in Rust. What was taken
from the reference implementations is knowledge about a network protocol —
byte offsets, opcode numbers, packet sizes — plus the cipher's 512-byte key
table, which is the protocol's key material and cannot be derived.

No game assets, client binaries or copyrighted content from Aika Online are
included or distributed here. Aika Online is the property of its respective
rights holders. This project is for educational purposes.
