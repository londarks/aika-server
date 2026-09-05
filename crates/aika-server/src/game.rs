//! Game server (port 8822 by default).
//!
//! This is where the client lands after login. It sends a `0x685` with the
//! account and version; the server answers `0x901` with the three character
//! slots, which is the selection screen.
//!
//! Reference: the dispatch that matters in the Delphi server is the one at
//! `Threads/PlayerThread.pas:97`. Two nearly identical copies exist
//! (`Connections/ServerSocket.pas:3307` and `Threads/UpdateThreads.pas:419`)
//! that are dead: neither has a live call site.

use crate::state::State;
use crate::inventory::{self, Inventory};
use crate::store::{Account, Character, Item, MAX_CHARACTERS};
use crate::{ability, combat, creation, dialog, expiry, pran, promotion, shop, stats};
use crate::world::{Outbox, DISTANCE_TO_FORGET, DISTANCE_TO_WATCH};
use crate::effects::Effects;
use aika_data::itemlist::ItemList;
use aika_data::skills::SkillTable;
use aika_data::npc::Npc;
use aika_net::frame::{self, FrameError, FrameReader, Message, MIN_FRAME};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

/// Client to server: enter the game server with an authenticated account.
pub const OP_REQUEST_LOGIN: u16 = 0x685;
/// Server to client: the three slots of the selection screen.
pub const OP_CHAR_LIST: u16 = 0x901;
/// Client to server: picked a character and wants to enter the world.
pub const OP_ENTER_WORLD: u16 = 0xF02;
/// Server to client: the whole character (`TSendToWorldPacket`).
pub const OP_SEND_TO_WORLD: u16 = 0x925;
/// Signals that precede `0x925`, in the order the original sends them.
pub const OP_SIGNAL_READY: u16 = 0xCCCC;
pub const OP_SIGNAL_LOAD: u16 = 0x186;
/// Client to server: finished loading, ready to spawn.
pub const OP_CLIENT_READY: u16 = 0xF0B;
/// Client to server: walked to a point. The server returns nothing to the
/// mover, it only relays to the others who can see them.
pub const OP_MOVE: u16 = 0x301;
/// `TSendClientIndexPacket`: which mob on screen is the player's own.
/// Without it the client has a world it cannot attach its camera to.
pub const OP_CLIENT_INDEX: u16 = 0x117;
/// Three more the original sends around the world packet. Their contents are
/// zero and their meaning is unknown; what is known is the order.
pub const OP_ENTER_3A2: u16 = 0x3A2;
pub const OP_ENTER_131: u16 = 0x131;
pub const OP_ENTER_12C: u16 = 0x12C;
pub const OP_ENTER_94C: u16 = 0x94C;

/// `TSignalData`: which way the player is facing.
pub const OP_ROTATE: u16 = 0x305;
/// `TMoveItemPacket`: drag an item from one slot to another.
pub const OP_MOVE_ITEM: u16 = 0x70F;
/// Throw an item away.
pub const OP_DELETE_ITEM: u16 = 0x32C;
/// `TUseItemPacket`: drink the potion in a slot.
pub const OP_USE_ITEM: u16 = 0x31D;
/// Leave the world and go back to the selection screen.
pub const OP_BACK_TO_CHARACTER_SELECT: u16 = 0x668;
/// Get up after dying.
pub const OP_REVIVE: u16 = 0x303;
/// `TClientMessagePacket` (`Data/Packets.pas:152`): a line of text on screen.
pub const OP_CLIENT_MESSAGE: u16 = 0x984;
/// `TChatPacket` (`Data/Packets.pas:188`), the packet a player sends to speak
/// and the one the server echoes back to everyone who should hear it. The
/// original dispatches it at `$F86` (`ServerSocket.pas:3696`).
pub const OP_CHAT: u16 = 0xF86;
/// `TChangeItemBarPacket` (`Data/Packets.pas:1118`): the player dragged
/// something onto, off, or across the action bar. The original both reads and
/// echoes it at `$31E` (`ServerSocket.pas:3626`, `RefreshItemBarSlot`).
pub const OP_CHANGE_ITEM_BAR: u16 = 0x31E;
/// The kind field of that packet, for the one kind the server sends by itself:
/// a skill (`RefreshItemBarSlot(BarIndex, 2, NewSkillIndex)`).
pub const ITEM_BAR_SKILL: u32 = 2;
/// `TAgroupItemPacket` (`Data/Packets.pas:887`): stack one pile onto another
/// (`AgroupItem`, `$332`).
pub const OP_GROUP_ITEM: u16 = 0x332;
/// `TUngroupItemPacket` (`Data/Packets.pas:895`): split a pile in two
/// (`UngroupItem`, `$333`).
pub const OP_UNGROUP_ITEM: u16 = 0x333;
/// `TSendActionPacket` (`Data/Packets.pas:441`): the player sat down, waved or
/// danced. Read and echoed at `$304` (`UpdateAction`,
/// `PacketHandlers.pas:1026`).
pub const OP_ACTION: u16 = 0x304;
/// `$202` (`RequestServerTime`, `PacketHandlers.pas:13038`): the player asked
/// what time it is where the server is.
pub const OP_SERVER_TIME: u16 = 0x202;
/// `TStoragePacket` (`Data/Packets.pas:985`): the whole chest in one packet,
/// the gold in it and all eighty-six slots (`SendStorage`, `Mob/Player.pas:4402`).
pub const OP_STORAGE: u16 = 0x137;
/// The signal that opens the chest window, carrying which chest it is
/// (`SendData(clientId, $310, StorageType)`).
pub const OP_STORAGE_OPEN: u16 = 0x310;
/// `TChangeChestGoldPacket` (`Data/Packets.pas:976`): move gold between the
/// purse and the chest. The amount is signed — out of the chest is negative.
pub const OP_CHEST_GOLD: u16 = 0xF59;
/// `TUseBuffItemPacket` (`Data/Packets.pas:1002`): a bag slot, and nothing
/// else. This is the packet the client sends for a saddle or a lasting potion
/// (`UseBuffItem`, `$21B`) — not the ordinary use-item one.
pub const OP_USE_BUFF_ITEM: u16 = 0x21B;
/// `TUpdateBuffPacket` (`Data/Packets.pas:1429`): one buff started, and when
/// it ends (`SendAddBuff`, `$16F`).
pub const OP_ADD_BUFF: u16 = 0x16F;
/// `TUseMountSkill` (`Data/Packets.pas:140`): one of the mount's own two
/// skills (`UseMountSkill`, `$218`).
pub const OP_MOUNT_SKILL: u16 = 0x218;
/// `TRemoveBuffPacket`: the player clicked a buff away (`RemoveBuff`,
/// `$329`). The body is the skill that started it.
pub const OP_REMOVE_BUFF: u16 = 0x329;
/// `TGetStatusPointPacket` (`Data/Packets.pas:2150`): the player spent free
/// points on an attribute (`GetStatusPoint`, `$213`).
pub const OP_STATUS_POINT: u16 = 0x213;
/// The player asked to unlearn everything and start the sheet again
/// (`ResetSkills`, `$32A`, `PlayerThread.pas:1015`). The packet carries
/// nothing: the request is the whole of it.
pub const OP_RESET_SKILLS: u16 = 0x32A;
/// Walking, the only move type the original relays to other players
/// (`Data/GlobalDefs.pas:216`). The real client also sends other values.
const MOVE_NORMAL: u8 = 0;
/// Server-originated teleport (`Data/GlobalDefs.pas:217`). A client must
/// never be able to move itself this way.
const MOVE_TELEPORT: u8 = 1;
/// How many movement packets to dump in full per connection, to confirm the
/// layout against the real client without flooding the log.
const MOVES_TO_DUMP: u8 = 3;
/// Server to client: create a creature on the map. This is the spawn.
pub const OP_CREATE_MOB: u16 = 0x349;
/// Server to client: take a creature off the map.
pub const OP_REMOVE_MOB: u16 = 0x101;

/// `TSendRemoveMobPacket`: the id leaving and why.
const REMOVE_MOB_SIZE: usize = 20;
/// `DELETE_DISCONNECT` in `Data/GlobalDefs.pas:216`. There is also 0 for a
/// plain disappearance, 1 for death and 3 for an unspawn effect.
const DELETE_DISCONNECT: u32 = 2;
/// `DELETE_NORMAL`: the creature simply goes off screen, no effect.
const DELETE_NORMAL: u32 = 0;

// The burst the original sends after `0xF0B`, in order. There is no ack
// packet: the client stops resending `0xF0B` once it has received enough of
// this to consider itself in the world (`Mob/Player.pas:4945-5244`).
/// Skill list.
pub const OP_SKILLS: u16 = 0x106;
/// `TSendSkillsLevelPacket` (`0x107`): which skills are learned and at what
/// level, plus how many skill points are unspent (`SendPlayerSkillsLevel`).
/// This is what tells the client a basic is castable and greys out the rest.
pub const OP_SKILLS_LEVEL: u16 = 0x107;
/// `TLearnSkillPacket` (`Data/Packets.pas:1439`): the player spent points at a
/// trainer to learn or rank up a skill (`LearnSkill`, `$31C`).
pub const OP_LEARN_SKILL: u16 = 0x31C;
/// Cash balance, carried as a plain signal.
pub const OP_CASH: u16 = 0x139;
/// Account status.
pub const OP_ACCOUNT_STATUS: u16 = 0x14F;
/// Active buffs.
pub const OP_BUFFS: u16 = 0x16E;
/// Active title.
pub const OP_ACTIVE_TITLE: u16 = 0x361;
/// Nation relics.
pub const OP_RELICS: u16 = 0x136;
/// Attribute points.
pub const OP_REFRESH_POINT: u16 = 0x109;
/// Combat stats.
pub const OP_REFRESH_STATUS: u16 = 0x10A;
/// Full attribute reply, which the original emits right after `0x10A`.
pub const OP_ALL_ATTRIBUTES: u16 = 0x23FF;
/// Level and experience.
pub const OP_LEVEL: u16 = 0x108;
/// Current and maximum HP/MP.
pub const OP_HP_MP: u16 = 0x103;

/// Several packets carry this fixed value in the header `Index` field instead
/// of the client id.
const FIXED_INDEX: u16 = 0x7535;

/// Total sizes, header included, taken from the Delphi records.
const SKILLS_SIZE: usize = 96;
const SIGNAL_SIZE: usize = 16;
const BUFFS_SIZE: usize = 252;
const ACTIVE_TITLE_SIZE: usize = 20;
const RELICS_SIZE: usize = 66;
const REFRESH_POINT_SIZE: usize = 28;
const REFRESH_STATUS_SIZE: usize = 54;
const ALL_ATTRIBUTES_SIZE: usize = 50;
const LEVEL_SIZE: usize = 24;
const HP_MP_SIZE: usize = 32;

/// The original writes a single `0xCC` byte into a WORD field of `0x108`
/// (`Mob/BaseMob.pas:2434`), so the field reads `CC 00`.
const LEVEL_UNK: u16 = 0x00CC;

/// `TSendActionPacket`: the header plus two dwords, the animation and whether
/// it repeats (`Data/Packets.pas:441`).
const ACTION_SIZE: usize = MIN_FRAME + 8;

/// Sizes of the world-entry packets, from their records in `Data/Packets.pas`.
const CLIENT_INDEX_SIZE: usize = 20;
const ENTER_3A2_SIZE: usize = 20;
const ENTER_131_SIZE: usize = 20;
const ENTER_12C_SIZE: usize = 96;
const ENTER_94C_SIZE: usize = 164;
/// The one value in any of them that is not zero (`Tp131.Unk_1`).
const ENTER_131_MARKER: u32 = 0xFFFF_FFFF;
/// The two storage slots the original refreshes on the way in. Storage is not
/// modelled yet, so they go out empty, which is what an untouched account
/// would have sent anyway.
const ENTER_STORAGE_SLOTS: [u16; 2] = [54, 55];

/// `TClientMessagePacket`, 144 bytes in total.
const CLIENT_MESSAGE_SIZE: usize = 144;
/// The text field inside it: 128 bytes, NUL terminated. The original walks it
/// as a plain array from index zero, writing over what would be a Delphi
/// short string's length byte, which tells us the client reads it as `char[]`
/// and not as a Delphi string.
const CLIENT_MESSAGE_TEXT: usize = 128;
/// Yellow, at the top of the screen: what the original passes by default.
const MESSAGE_NOTICE: u8 = 16;

/// Item types a potion can be (`Data/GlobalDefs.pas:845,862,863,870`).
const ITEM_TYPE_TEARS: u16 = 29;
const ITEM_TYPE_HP_POTION: u16 = 700;
const ITEM_TYPE_MP_POTION: u16 = 701;
const ITEM_TYPE_HPMP_POTION: u16 = 800;
/// The item that opens the chest wherever the player is standing
/// (`ITEM_TYPE_STORAGE_OPEN`, `Data/GlobalDefs.pas:856`).
const ITEM_TYPE_STORAGE_OPEN: u16 = 226;
/// The three kinds of item whose whole job is to start a buff
/// (`Data/GlobalDefs.pas:864`). A lasting potion is used with the ordinary
/// use-item packet; a saddle and its cousins come in on `0x21B` instead.
const ITEM_TYPE_POTION_BUFF: u16 = 702;
const ITEM_TYPE_BUFF: u16 = 715;
const ITEM_TYPE_BUFF2: u16 = 716;

/// `TSendBuffsPacket`: forty buff ids and forty end times.
const BUFFS_COUNT: usize = crate::buffs::MAX_BUFFS;
const BUFFS_TIMES_AT: usize = BUFFS_COUNT * 2;
/// `TUpdateBuffPacket`: the buff, when it ends, and a spare dword.
const ADD_BUFF_SIZE: usize = MIN_FRAME + 12;
/// `TUseMountSkill`: one byte saying which of the two.
const MOUNT_SKILL_SIZE: usize = MIN_FRAME + 2;

/// The two skills a mount carries, picked by the byte the client sends
/// (`UseMountSkill`: nought is one of them, one is the other).
const MOUNT_SKILLS: [usize; 2] = [6986, 6987];
/// Only prans go in the last two chest slots, and this is what one is
/// (`MoveItem` checks `ItemType = 10`).
const ITEM_TYPE_PRAN: u16 = 10;

/// Which chest a packet is about (`Data/GlobalDefs.pas:363`). Two is the
/// player's own; three is the guild's, which waits on guilds.
/// What goes in the header's time field: nothing.
///
/// The original never puts anything there. `Mob/Player.pas` and
/// `Mob/BaseMob.pas` build almost every packet the server sends and assign
/// `Header.Time` exactly zero times between them -- they `ZeroMemory` the
/// record and fill in the size, the index and the code. The sixteen places
/// that do touch it in `PacketHandlers.pas` assign 0, with
/// `Header.Time := GetTickCount` commented out on the line below each one.
/// Somebody tried a real clock there and put it back.
///
/// This used to carry milliseconds since the server process started, which is
/// a number the client has no way to relate to its own clock. It reads the
/// field: an equipment change would be refused with "you may equip in N
/// seconds", a different N for each item and a different one every session,
/// counting down to a deadline that came from our uptime rather than from
/// anything the player did.
const PACKET_TIME: u32 = 0;

const STORAGE_TYPE_PLAYER: u32 = 1;
/// The same window told to open on the pran side of itself
/// (`STORAGE_TYPE_PRANS`, `Data/GlobalDefs.pas:364`).
const STORAGE_TYPE_PRANS: u32 = 2;
const CHEST_TYPE_STORAGE: u32 = 2;

/// What the original marks as the window being open: option seven is the
/// chest (`ITEM_TYPE_STORAGE_OPEN` sets `OpennedOption := 7`).
const OPTION_STORAGE: u32 = 7;
/// The Pran station opens the same chest, so a move out of it has to be
/// allowed on the same terms.
const OPTION_PRAN_STATION: u32 = 13;

/// The most gold either the purse or the chest will hold. The original
/// refuses a transfer that would push past it rather than wrapping round.
const GOLD_CAP: u64 = 2_000_000_000;

/// `TStoragePacket`: a dword the original leaves at zero, the gold, and the
/// eighty-six slots.
const STORAGE_SIZE: usize = MIN_FRAME
    + 4
    + 8
    + inventory::STORAGE_SLOTS as usize * character_offset::ITEM_SIZE;
/// `TChangeChestGoldPacket`: which chest, and how much in or out.
const CHEST_GOLD_SIZE: usize = MIN_FRAME + 4 + 8;

/// The index the original stamps on a menu entry (`NPCHandlers.pas:172`).
/// Note that it is not the `0x7535` used everywhere else — the digits are
/// swapped, and copying the wrong one puts the menu on nobody's screen.
const MENU_ENTRY_INDEX: u16 = 0x3575;

/// `TSendCreateMobPacket` (`Data/Packets.pas:344`), 508 bytes in total.
const CREATE_MOB_SIZE: usize = 508;

/// Offsets inside the `0x349` body (the 12-byte header is already gone).
mod spawn_offset {
    pub const NAME: usize = 0;
    pub const EQUIP: usize = 16;
    /// The stone and the mount, which are not in the equip array above: it
    /// holds eight slots and they live in nine and ten. The original copies
    /// their item ids into two fields of their own
    /// (`ItemEffPedra`/`ItemEffMontaria`, `Mob/BaseMob.pas:3078`), and this is
    /// the only place the client is told what somebody is riding.
    pub const ITEM_EFF_STONE: usize = 32;
    pub const ITEM_EFF_MOUNT: usize = 34;
    pub const ARMA_REFINE: usize = 39;
    /// The position is a pair of `f32`, not integers, and there is no Z.
    pub const POSITION_X: usize = 44;
    pub const POSITION_Y: usize = 48;
    pub const ROTATION: usize = 52;
    pub const MAX_HP: usize = 56;
    pub const MAX_MP: usize = 60;
    pub const CUR_HP: usize = 64;
    pub const CUR_MP: usize = 68;
    pub const UNK0: usize = 72;
    pub const SPEED_MOVE: usize = 73;
    pub const SPAWN_TYPE: usize = 74;
    pub const SIZES: usize = 75;
    /// Where the companion's own client id sits, in the fourth byte of
    /// what is four bytes of build on a character and three on a pran
    /// (`TSendCreatePranPacket`, `Data/Packets.pas:380`).
    pub const PRAN_CLIENT_ID: usize = SIZES + 3;
    pub const GUILD_AND_NATION: usize = 476;
    /// Four WORDs; index 1 carries a fixed constant.
    pub const EFFECTS: usize = 478;
    pub const BODY_SIZE: usize = 496;
}

/// Extra fields the NPC flavour of `0x349` uses (`Mob/BaseNpc.pas:1855`).
mod npc_offset {
    /// Where the job title goes, the line the client draws under the name.
    pub const TITLE: usize = 444;
    pub const TITLE_MAX: usize = 32;
    /// One byte right after the four sizes.
    pub const IS_SERVICE: usize = 79;
    pub const EFFECT_TYPE: usize = 80;
}

/// An NPC is not a player: it is flagged as a service, carries a different
/// `Unk0`, and lights up effect type 1 (`Mob/BaseNpc.pas:1886`).
const NPC_UNK0: u8 = 0x28;
const NPC_IS_SERVICE: u8 = 1;
const NPC_EFFECT_TYPE: u16 = 1;

/// The equipment slots the spawn packet's equip array carries beyond the body
/// and the hair. It holds eight and stops there.
const WORN_SLOTS: std::ops::Range<u16> = 2..8;

/// The two slots past that array, which the spawn carries in fields of their
/// own: the stone whose effect glows on the gear, and the mount the character
/// is riding. A mount is an item like any other — type 9 in the table — and
/// putting one here is what puts the player on it.
const STONE_SLOT: u16 = 8;
const MOUNT_SLOT: u16 = 9;

/// The item id in an equipment slot, or zero for an empty one. Unlike
/// [`worn_appearance`] there is no appearance override here, because the
/// original copies the index for these two and nothing else.
fn worn_index(character: &Character, slot: u16) -> u16 {
    character.items.get(inventory::EQUIP, slot).map_or(0, |item| item.index)
}

/// What the client should draw in one equipment slot.
///
/// The appearance wins when there is one, which is how a piece of gear can
/// look like something else; otherwise the item itself
/// (`Mob/BaseNpc.pas:1876`).
fn worn_appearance(character: &Character, slot: u16) -> u16 {
    let Some(item) = character.items.get(inventory::EQUIP, slot) else {
        return 0;
    };
    if item.appearance != 0 {
        item.appearance
    } else {
        item.index
    }
}

/// Fixed values the original writes without explanation, but which the
/// client expects (`Mob/BaseMob.pas:2974-3131`).
const SPAWN_UNK0: u8 = 0x0A;
const SPAWN_EFFECT_1: u16 = 0x1D;
const SPAWN_ARMA_REFINE: u8 = 15;
/// Spawn types (`Data/GlobalDefs.pas:211`): 0 is somebody who was already
/// there coming into view, 1 is somebody arriving or teleporting in. The
/// client draws them differently, so a player logging in next to you should
/// not look like one who was standing there all along.
const SPAWN_TELEPORT_IN: u8 = 1;

/// The send type the original stamps on a skill list that came from a
/// trainer rather than from arriving in the world (`NPCHandlers.pas:7119`).
const SKILL_LIST_FROM_NPC: u16 = 0x0B;

/// What the original gives every monster (`ServerSocket.pas:655`).
const MOB_SPEED_MOVE: u8 = 22;
/// `SPAWN_NORMAL` in `Data/GlobalDefs.pas:211`. There is also 1 (teleport)
/// and 2 (offspring birth); bit 0x80 is added when the player is in PK.
const SPAWN_NORMAL: u8 = 0;

/// Size of `TSendToCharListPacket`.
const CHAR_LIST_SIZE: usize = 336;
/// Size of each `TCharacterListData`.
const CHAR_ENTRY_SIZE: usize = 104;

/// Size of `TCharacter` (`Data/PlayerData.pas:192`), summing the declared
/// types: `TStatus` = 140, `TItem` = 20, `TQuest` = 12.
///
/// The offset comments inside the record are stale (they disagree with each
/// other by 16 and 32 bytes): trust the declared types, never the comments.
/// The round 6400 total for the whole packet is a good sign the sum adds up.
const CHARACTER_SIZE: usize = 6384;
/// `TSendToWorldPacket`: header, account serial and the character.
const SEND_TO_WORLD_SIZE: usize = MIN_FRAME + 4 + CHARACTER_SIZE;
/// The original writes this fixed value in the `Index` header field of the
/// `0x925`, instead of the client id (`Mob/Player.pas:3300`).
const SEND_TO_WORLD_INDEX: u16 = 0x7535;

/// Offsets inside `TCharacter`.
mod character_offset {
    pub const CLIENT_ID: usize = 0;
    pub const FIRST_LOGIN: usize = 4;
    pub const CHAR_INDEX: usize = 8;
    pub const NAME: usize = 12;
    pub const NATION: usize = 28;
    pub const CLASS_INFO: usize = 29;
    /// Start of `TStatus`.
    pub const SCORE: usize = 32;
    pub const ATTRIBUTES: usize = SCORE;
    pub const SIZES: usize = SCORE + 12;
    pub const MAX_HP: usize = SCORE + 16;
    pub const CUR_HP: usize = SCORE + 20;
    pub const MAX_MP: usize = SCORE + 24;
    pub const CUR_MP: usize = SCORE + 28;
    /// `CurrentScore.SkillPoint`, a word inside `TStatus`. After the four
    /// vitals dwords come five more (server reset, honor, kill points, infamy)
    /// and the two-byte evil points, which puts it here
    /// (`Data/PlayerData.pas:165`). The skill window reads the unspent count
    /// from the record, not only from `0x107`, which is why setting it in the
    /// packet alone left the window showing zero.
    pub const SKILL_POINT: usize = SCORE + 50;
    pub const EXP: usize = 176;
    pub const LEVEL: usize = 184;
    /// 16 items of 20 bytes; slot 0 is the class and slot 1 the hair, so
    /// real equipment starts at slot 2.
    pub const EQUIP: usize = 340;
    /// 126 items of 20 bytes. The comment beside the field says 60, but
    /// `EQUIP + 16 * 20 + 4 + 126 * 20` lands exactly on the gold, which is
    /// the arithmetic that settles it.
    pub const INVENTORY: usize = EQUIP + 16 * ITEM_SIZE + 4;
    /// `TItem` (`Data/MiscData.pas:44`).
    pub const ITEM_SIZE: usize = 20;
    pub const GOLD: usize = 3184;
    pub const LOCATION: usize = 3792;
    /// Sixty words: the skills the character knows (`SkillList`).
    pub const SKILL_LIST: usize = 4596;
    /// Forty dwords: the action bar (`ItemBar`). This is where the hotbar
    /// icons live, so a character with an empty one here logs in with a bare
    /// bar however many skills it knows.
    pub const ITEM_BAR: usize = 4716;
    /// The names of the account's two companions, sixteen bytes each.
    ///
    /// They are in the *character* record, which is the last place anybody
    /// looks for them: `Move(Pran1.Name, Packet.Character.PranName[0], 16)`
    /// (`Mob/Player.pas:3421`). So a client learns what a pran is called when
    /// it enters the world, and nowhere else -- the pran's own packet has no
    /// name field the client trusts for this, and until this was filled in the
    /// client asked the player to name an already-named pran, refused to let
    /// it out of the chest, and could not be argued out of either.
    ///
    /// The offset is counted from the end rather than from the start. This is
    /// the last field of `TCharacter` bar a trailing dword, and counting 260
    /// fields forward through a record whose own offset comments disagree with
    /// each other is how you get this wrong.
    pub const PRAN_NAMES: usize = super::CHARACTER_SIZE - 4 - 2 * PRAN_NAME_SIZE;
    pub const PRAN_NAME_SIZE: usize = 16;
}

pub async fn serve(state: Arc<State>, listener: TcpListener) -> anyhow::Result<()> {
    info!(addr = %listener.local_addr()?, "game server listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            debug!(%peer, "client connected to the game server");
            if let Err(e) = handle_connection(state, stream).await {
                debug!(%peer, error = %e, "game connection closed");
            }
        });
    }
}

/// State of one game connection: who logged in and with which character.
///
/// It lives here, per connection, rather than in a global, which is what
/// allows two players at once. Seeing each other will need a shared
/// registry on top of this, but the connection owns the session.
#[derive(Default)]
pub(crate) struct Session {
    account: Option<Account>,
    character: Option<Character>,
    /// Id the client uses to refer to itself in packets.
    client_id: u16,
    /// How many movement packets were dumped in full so far.
    moves_logged: u8,
    /// Players this connection has already been shown. Kept per connection so
    /// that walking into and out of range spawns and removes them exactly once.
    visible: std::collections::HashSet<u16>,
    /// The character already spawned. The client resends `0xF0B` whenever it
    /// thinks something is missing, and spawning again teleports the player to the
    /// starting point, which is the original's `if IsInstantiated then Exit`
    /// (`Mob/Player.pas:4967`).
    spawned: bool,
    /// NPCs already placed on this player's screen. Same reason as `visible`:
    /// sending a spawn twice makes the client draw two of them.
    visible_npcs: HashSet<u16>,
    /// Which of the account's companions is out, if any. A pran in the
    /// chest earns nothing, which is the original's rule: it hands out a
    /// share inside a switch on `SpawnedPran`.
    pran_out: Option<usize>,
    /// Whether the player has already been told the pran is at a wall, so
    /// they are told once rather than on every kill.
    pran_told_to_evolve: bool,
    /// The id a companion with a body is being drawn under, if one is out.
    ///
    /// Kept because dismissing has to undo whatever summoning did, and by
    /// the time the stone comes off there is nothing left to ask. A pran
    /// drawn as an effect is cleared with an effect and leaves this `None`.
    pran_body: Option<u32>,
    /// Which way the player is facing, so a repeat can be dropped.
    rotation: u32,
    /// What the player is doing that outlives the packet: sitting, or the
    /// dance it was given. Kept beside the world's copy for the same reason
    /// the rotation is — it is what gets sent to somebody walking into view.
    action: u32,
    /// Health and mana as they stand. Not on the character because nothing
    /// takes them down yet; they live here so a potion has somewhere to go.
    cur_hp: u32,
    cur_mp: u32,
    /// Down, and waiting to get up. A dead character takes no more damage
    /// and cannot swing.
    dead: bool,
    /// Something changed that the database has not been told about yet.
    dirty: bool,
    /// When the last write went out, so a player walking in a straight line
    /// does not write a row per step.
    saved_at: Option<std::time::Instant>,
    /// When each spell this character has cast may be cast again. On the
    /// session rather than the character: a cooldown that survived a logout
    /// would be a reason to log out.
    cooldowns: ability::Cooldowns,
    /// Monsters already on this player's screen, for the same reason as the
    /// players and the NPCs: sending a spawn twice draws two of them.
    visible_mobs: HashSet<u16>,
    /// Which NPC has a window open, if any. Every shop packet is checked
    /// against it: a client that never opened a shop must not be able to buy
    /// from one, and the original refuses on the same grounds.
    opened_npc: Option<u16>,
    /// Which window it is (`OpennedOption`). The chest is seven, and putting
    /// something into it is refused unless this says the chest is open — the
    /// original's own guard, and the reason it is kept rather than inferred
    /// from the NPC alone: an item opens the chest with no NPC involved.
    opened_option: u32,
    /// What is currently working on this character: potions, blessings, and
    /// the one that says they are on a mount. It lives on the session because
    /// a buff is measured in minutes and would mean nothing after a logout,
    /// which is where the original keeps it too.
    buffs: crate::buffs::Buffs,
    /// When the buff list last went out. An endless buff is drawn from a
    /// window that counts down, so it has to be sent again before the
    /// window empties or the icon leaves the bar on its own.
    buffs_sent_at: Option<std::time::Instant>,
    /// The last rank bought, which may not be bought again
    /// (`TPlayer.SkillUpgraded`). Per connection in the original too, so
    /// logging out clears it.
    skill_upgraded: Option<usize>,
}

impl Session {
    /// Everything working on this character: its buffs and what it wears.
    ///
    /// Worked out on demand rather than kept, because a buff runs out on its
    /// own and a stored copy would go stale without anything saying so.
    fn effects(&self, state: &State) -> Effects {
        match self.character.as_ref() {
            Some(character) => {
                Effects::of(character, &state.items, &self.buffs, &state.skills)
            }
            None => Effects::none(),
        }
    }
}

async fn handle_connection(state: Arc<State>, stream: TcpStream) -> anyhow::Result<()> {
    let (mut incoming, mut outgoing) = stream.into_split();
    let (outbox, mut queue) = mpsc::unbounded_channel::<Vec<u8>>();

    // Shared with the writer, because the writer is the only place every
    // outbound frame passes through: a reply and a broadcast from somebody
    // else's connection both end up in that queue.
    let trace = Arc::new(Mutex::new(crate::trace::Trace::new(Instant::now())));

    // The id is ours to hand out, not the client's to claim: the client learns
    // it from the packets we send, and echoing back whatever it sent would give
    // every player the same one.
    let Some(client_id) = state.world.connect(outbox.clone()) else {
        warn!(players = state.world.online(), "refused a connection: the channel is full");
        return Ok(());
    };

    // One task owns the write half, so a broadcast from another connection can
    // reach this player without either side waiting on the other.
    let writer = tokio::spawn({
        let trace = Arc::clone(&trace);
        async move {
            while let Some(frame) = queue.recv().await {
                {
                    let mut trace = trace.lock().await;
                    trace.sent_to_client(&frame, Instant::now());
                    if trace.is_live() {
                        debug!("{}", trace.last_line());
                    }
                }
                if outgoing.write_all(&frame).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut session = Session { client_id, ..Session::default() };
    let result = read_loop(&state, &mut session, &outbox, &mut incoming, &trace).await;

    // Saved before anything else in the teardown: whatever went wrong with
    // the connection, where the player stood and what it was carrying are
    // still worth keeping.
    autosave(&state, &mut session, true).await;

    leave_world(&state, &session);
    writer.abort();
    result
}

/// How often monsters are brought back, which nothing else is measured
/// against. A second is finer than any respawn in the shipped data, the
/// shortest of which is thirty.
const RESPAWN_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// Runs the monsters, on the two clocks the original runs them on.
///
/// `ServerSocket.pas:876` starts two threads and gives them two periods:
/// `TMobHandlerThread1` every second decides who swings, and
/// `TMobMovimentThread1` every three seconds decides where everything
/// stands. Both numbers are behaviour, not tuning — running the movement on
/// a fast clock is what made monsters sprint across the field, because each
/// turn shifts a fixed distance rather than a distance per second.
///
/// The players are not told about respawns from here. Each connection keeps
/// its own idea of what is on its screen, and pushing a spawn from outside
/// would desync it; instead the session notices on its next refresh, which
/// the client's own twice-a-second heartbeat drives.
pub fn spawn_world_tick(state: Arc<State>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut respawns = tokio::time::interval(RESPAWN_TICK);
        let mut fights = tokio::time::interval(crate::mob::COMBAT_TICK);
        let mut moves = tokio::time::interval(crate::mob::MOVE_TICK);

        loop {
            let now = tokio::select! {
                _ = respawns.tick() => {
                    let revived = state.world.revive_mobs(std::time::Instant::now());
                    if !revived.is_empty() {
                        debug!(count = revived.len(), "monsters came back");
                    }
                    continue;
                }
                _ = fights.tick() => Clock::Combat,
                _ = moves.tick() => Clock::Movement,
            };

            // Nothing to think about with nobody in the world, and several
            // thousand monsters pacing for an empty map is work for nothing.
            let players = state.world.positions();
            if players.is_empty() {
                continue;
            }

            match now {
                Clock::Combat => run_combat(&state, &players),
                Clock::Movement => run_movement(&state, &players),
            }
        }
    })
}

/// Which of the two monster threads a turn of the loop is.
enum Clock {
    Combat,
    Movement,
}

/// `TMobSPosition.MobHandler`: who swings at whom, and who closes in.
fn run_combat(state: &Arc<State>, players: &[(u16, (f32, f32))]) {
    let (blows, moved) = state.world.fight_mobs(players, std::time::Instant::now());

    for attack in blows {
        debug!(
            mob = attack.attacker,
            target = attack.target,
            damage = attack.damage,
            skill = attack.skill,
            "a monster swung"
        );
        state.world.deal_to_player(attack.target, attack);
    }
    tell_watchers(state, players, moved);
}

/// `TMobSPosition.MobMoviment`: where everything stands.
fn run_movement(state: &Arc<State>, players: &[(u16, (f32, f32))]) {
    let moved = state.world.move_mobs(players, std::time::Instant::now());
    tell_watchers(state, players, moved);
}

/// Tells everyone who can see a monster where it has got to.
///
/// The original sends where the monster now *is*, having already moved it
/// there, and the client walks it the whole way at the speed in the packet.
/// Sending a short step on a fast clock instead is what made monsters stutter:
/// the client kept arriving early and standing still until the next one.
fn tell_watchers(
    state: &Arc<State>,
    players: &[(u16, (f32, f32))],
    moved: Vec<(crate::mob::Mob, crate::mob::Turn)>,
) {
    for (mob, turn) in moved {
        let Some(to) = turn.walk else { continue };
        let frame = encode_mob_walk(&mob, to, turn.speed);
        for (id, at) in players {
            if within(*at, mob.position(), DISTANCE_TO_WATCH) {
                state.world.send_to(*id, frame.clone());
            }
        }
    }
}

/// `0x301` for a monster: `TBaseMob.WalkTo`.
///
/// It carries where the monster is *going*, not where it is, and the speed to
/// get there at. The client walks it the whole way on its own — which is why
/// sending a short step every tick instead made monsters jump: they arrived
/// early and stood still until the next packet.
fn encode_mob_walk(mob: &crate::mob::Mob, to: (f32, f32), speed: u8) -> Vec<u8> {
    let mut body = vec![0u8; Movement::BODY_SIZE];
    body[0..4].copy_from_slice(&to.0.to_le_bytes());
    body[4..8].copy_from_slice(&to.1.to_le_bytes());
    body[Movement::MOVE_TYPE] = MOVE_NORMAL;
    body[Movement::SPEED] = speed;

    frame::encode(
        &Message { sender: mob.id, opcode: OP_MOVE, time: 0, body },
        rand::random(),
    )
}

/// Writes the session out if it owes anything and enough time has passed.
///
/// Saving only on disconnect is not enough: a crash, a power cut or a kill
/// takes the whole session with it, and to the player that is
/// indistinguishable from a database that does not work. The interval is a
/// trade — short enough that losing it costs a few steps, long enough that
/// walking across a map is a handful of writes rather than a hundred.
/// `force` ignores the clock, which is what a disconnect wants.
async fn autosave(state: &State, session: &mut Session, force: bool) {
    if !session.dirty {
        return;
    }
    let due = session
        .saved_at
        .is_none_or(|at| at.elapsed() >= state.autosave_every());
    if !force && !due {
        return;
    }

    let Some(character) = session.character.as_ref() else {
        session.dirty = false;
        return;
    };

    state.save_session(character).await;
    // And the chest, which the same edits reach: an item dragged out of the
    // bag and into it changes both sides at once.
    if let Some(account) = session.account.as_ref() {
        state.save_storage(account).await;
    }
    session.dirty = false;
    session.saved_at = Some(std::time::Instant::now());
}

async fn read_loop(
    state: &State,
    session: &mut Session,
    outbox: &Outbox,
    incoming: &mut tokio::net::tcp::OwnedReadHalf,
    trace: &Arc<Mutex<crate::trace::Trace>>,
) -> anyhow::Result<()> {
    let mut reader = FrameReader::new();
    let mut prefix = LeadingPrefix::default();
    let mut buf = [0u8; 8192];

    loop {
        // The read is given a deadline rather than being waited on for ever,
        // which is the only way a server notices a client that has stopped:
        // a frozen one does not disconnect, it just goes quiet.
        let n = match tokio::time::timeout(crate::trace::QUIET, incoming.read(&mut buf)).await {
            Ok(read) => read?,
            Err(_) => {
                let now = Instant::now();
                let mut trace = trace.lock().await;
                if trace.has_stopped(now) {
                    warn!("{}", trace.report(now));
                }
                continue;
            }
        };
        if n == 0 {
            return Ok(());
        }

        if let Some(data) = prefix.feed(&buf[..n]) {
            reader.push(data);
        }

        while let Some(message) = reader.next_message() {
            match message {
                Ok(message) => {
                    let action = handle_message(state, session, &message).await;
                    autosave(state, session, false).await;

                    // Recorded before the frames go out, so the order in the
                    // ring is the order on the wire.
                    {
                        let answered =
                            matches!(&action, Action::Reply(frames) if !frames.is_empty());
                        let mut trace = trace.lock().await;
                        trace.heard_from_client(
                            message.opcode,
                            &message.body,
                            answered,
                            Instant::now(),
                        );
                        if session.character.is_some() {
                            trace.entered_the_world();
                        }
                        if trace.is_live() {
                            debug!("{}", trace.last_line());
                        }
                    }

                    match action {
                        Action::Reply(frames) => {
                            for frame in frames {
                                let _ = outbox.send(frame);
                            }
                        }
                        Action::Ignore => {}
                        Action::Disconnect => return Ok(()),
                    }
                }
                Err(FrameError::BadChecksum) => {
                    warn!("game packet with invalid checksum");
                    return Ok(());
                }
                Err(FrameError::BadLength(size)) => {
                    warn!(size, "game packet with invalid size");
                    return Ok(());
                }
            }
        }
    }
}

/// Takes the player out of the registry and off everyone else's screen.
///
/// The watchers are collected before the removal, because once the player is
/// gone from the registry there is no position left to search around.
fn leave_world(state: &State, session: &Session) {
    let watchers = state.world.visible_to(session.client_id);
    let left = state.world.disconnect(session.client_id);

    if let (Some(character), false) = (session.character.as_ref(), watchers.is_empty()) {
        let frame = encode_remove_mob(session.client_id, DELETE_DISCONNECT);
        for watcher in &watchers {
            watcher.send(frame.clone());
        }
        info!(
            character = %character.name,
            watchers = watchers.len(),
            "left the world"
        );
    }
    let _ = left;
}

/// `TSendRemoveMobPacket` (`0x101`): who is leaving and why.
fn encode_remove_mob(client_id: u16, delete_type: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&(client_id as u32).to_le_bytes());
    body.extend_from_slice(&delete_type.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, REMOVE_MOB_SIZE);
    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REMOVE_MOB, time: 0, body },
        rand::random(),
    )
}

/// The first `recv` of a game connection carries 4 leading bytes that are
/// not part of the packet. The Delphi server cuts those 4 bytes blindly
/// (`PlayerThread.pas:2545-2570`) and only on the first read; here the call
/// is made from the content, so the server keeps working with a client that
/// does not send the prefix, ours for instance.
#[derive(Default)]
struct LeadingPrefix {
    decided: bool,
    pending: Vec<u8>,
}

impl LeadingPrefix {
    /// Returns the bytes to hand to the frame reader, or `None` while
    /// there are not enough bytes to decide yet.
    fn feed<'a>(&'a mut self, data: &'a [u8]) -> Option<&'a [u8]> {
        if self.decided {
            return Some(data);
        }

        self.pending.extend_from_slice(data);
        // We need the size field of both hypotheses to choose.
        if self.pending.len() < 6 {
            return None;
        }

        self.decided = true;
        let skip = if plausible_size(&self.pending) { 0 } else { 4 };
        // The first bytes of each connection tell whether the client really sends
        // the prefix, and which one. Worth logging while that is not yet
        // confirmed against the real client.
        let head: Vec<String> =
            self.pending.iter().take(8).map(|b| format!("{b:02X}")).collect();
        debug!(head = %head.join(" "), dropped = skip, "first packet of the connection");
        Some(&self.pending[skip.min(self.pending.len())..])
    }
}

/// A frame's leading size field only makes sense within a range; outside
/// it, whatever sits in front is not a frame.
fn plausible_size(data: &[u8]) -> bool {
    let size = u16::from_le_bytes([data[0], data[1]]) as usize;
    (MIN_FRAME..=8192).contains(&size)
}

/// What to do with the connection after handling a packet.
#[derive(Debug)]
pub(crate) enum Action {
    /// One or more frames to write, in order.
    Reply(Vec<Vec<u8>>),
    /// Packet recognised but no reply; the connection stays alive.
    Ignore,
    /// Login refused. The original server drops the connection on every
    /// refusal of `0x685`, and the client counts on that to show the error
    /// message instead of waiting forever.
    Disconnect,
}

/// One packet in, whatever should go out.
///
/// Asynchronous because some packets have to reach the database before they
/// can be answered: a character that exists in memory but not on disk is a
/// character that disappears at the next restart. The handlers that need
/// nothing from the database stay synchronous and are called directly.
async fn handle_message(state: &State, session: &mut Session, message: &Message) -> Action {
    match message.opcode {
        OP_REQUEST_LOGIN => handle_request_login(state, session, message),
        OP_ENTER_WORLD => handle_enter_world(state, session, message, PACKET_TIME),
        OP_CLIENT_READY => handle_client_ready(state, session),
        OP_MOVE => handle_move(state, session, message),
        dialog::OP_OPEN_NPC => handle_open_npc(state, session, message),
        dialog::OP_CLOSE_NPC_OPTION => handle_close_npc(session),
        shop::OP_BUY => handle_buy(state, session, message),
        shop::OP_SELL => handle_sell(state, session, message),
        OP_ROTATE => handle_rotate(state, session, message),
        OP_BACK_TO_CHARACTER_SELECT => handle_back_to_character_select(session),
        creation::OP_DELETE_CHARACTER | creation::OP_DELETE_CHARACTER_ALT => {
            handle_delete_character(state, session, message).await
        }
        creation::OP_CREATE_CHARACTER => {
            handle_create_character(state, session, message).await
        }
        OP_MOVE_ITEM => handle_move_item(state, session, message),
        OP_USE_ITEM => handle_use_item(state, session, message),
        combat::OP_ATTACK => handle_attack(state, session, message),
        OP_REVIVE => handle_revive(state, session, message),
        ability::OP_USE_SKILL => handle_use_skill(state, session, message),
        OP_DELETE_ITEM => handle_delete_item(session, message),
        OP_CHAT => handle_chat(state, session, message),
        OP_CHANGE_ITEM_BAR => handle_change_item_bar(session, message),
        OP_GROUP_ITEM => handle_group_item(state, session, message),
        OP_UNGROUP_ITEM => handle_ungroup_item(state, session, message),
        OP_LEARN_SKILL => handle_learn_skill(state, session, message),
        OP_ACTION => handle_action(state, session, message),
        OP_SERVER_TIME => handle_server_time(session),
        OP_CHEST_GOLD => handle_chest_gold(session, message),
        OP_USE_BUFF_ITEM => handle_use_buff_item(state, session, message),
        pran::OP_RENAME => handle_rename_pran(state, session, message).await,
        OP_MOUNT_SKILL => handle_mount_skill(state, session, message),
        OP_STATUS_POINT => handle_status_point(state, session, message),
        OP_RESET_SKILLS => handle_reset_skills(state, session),
        OP_REMOVE_BUFF => handle_remove_buff(state, session, message),
        opcode => {
            // The original merely prints the code here; we do the same, adding the
            // size alongside, to help identify the packet.
            // The body goes in the log too. Working out what an unknown
            // packet means starts with seeing what it carries, and asking a
            // person to reproduce it a second time is a wasted round trip.
            warn!(
                opcode = format!("0x{opcode:03x}"),
                size = message.body.len(),
                body = %hex_dump(&message.body),
                "game packet not implemented yet"
            );
            Action::Ignore
        }
    }
}

fn handle_request_login(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = RequestLogin::parse(&message.body) else {
        warn!(
            size = message.body.len(),
            minimo = RequestLogin::MIN_BODY,
            body = %hex_dump(&message.body),
            "0x685 packet too short"
        );
        return Action::Disconnect;
    };

    debug!(
        account = request.account_id,
        user = %request.username,
        version = request.version,
        "0x685 received"
    );

    let expected = state.cfg.game.client_version;
    if request.version != expected {
        // A wrong version usually means a wrong offset, not an old client:
        // the dump shows where the fields really are.
        warn!(
            version = request.version,
            expected,
            user = %request.username,
            body = %hex_dump(&message.body),
            "client with a different version"
        );
        return Action::Disconnect;
    }

    let Some(account) = state.store.get(&request.username) else {
        warn!(user = %request.username, "account not found on the game server");
        return Action::Disconnect;
    };

    if account.account_status == 8 {
        warn!(user = %account.username, "account blocked");
        return Action::Disconnect;
    }

    info!(
        user = %account.username,
        id = account.id,
        characters = account.characters.len(),
        "entered the game server"
    );

    // The client sends an id of its own in the header; ours is the one that
    // counts, and logging both is how a disagreement would surface.
    if message.sender != session.client_id {
        debug!(
            claimed = message.sender,
            assigned = session.client_id,
            "client claimed a different id"
        );
    }

    let frame = encode_char_list(&account, session.client_id, PACKET_TIME);
    state.world.set_account(session.client_id, account.id);
    session.account = Some(account);
    Action::Reply(vec![frame])
}

/// `0xF02`: the player picked a character and asked to enter.
///
/// The record is called `TNumericTokenPacket` in the original and suggests
/// a numeric password, but all that logic sits behind an unreachable
/// `Exit;` (`PacketHandlers.pas:484`): in practice only `Slot` is read.
fn handle_enter_world(
    state: &State,
    session: &mut Session,
    message: &Message,
    time: u32,
) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0xF02 packet without the slot");
        return Action::Disconnect;
    }
    let slot = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as usize;

    let Some(account) = session.account.clone() else {
        warn!("0xF02 before login; the connection has no account");
        return Action::Disconnect;
    };

    let Some(mut character) = account.characters.iter().find(|c| c.slot == slot).cloned() else {
        warn!(slot, user = %account.username, "empty slot");
        return Action::Disconnect;
    };

    // Every class is born knowing its six basic skills, so a character with an
    // empty skill list is one saved before the list was computed rather than
    // one that truly knows nothing. Mark the basics learned on the way in so
    // an old character can cast its basic attack without being remade — the
    // same `2` markers `SetPlayerSkills` writes.
    if character.skill_list[..BASIC_SKILL_COUNT].iter().all(|&s| s == 0) {
        for marker in &mut character.skill_list[..BASIC_SKILL_COUNT] {
            *marker = BASIC_SKILL_LEARNED;
        }
    }

    info!(
        user = %account.username,
        character = %character.name,
        slot,
        level = character.level,
        skill_points = character.skill_points,
        "entering the world"
    );

    let client_id = session.client_id;

    // The exact order the original sends (`Mob/Player.pas:3642-3657`). It is
    // not decoration: the client will not finish arriving until it has all of
    // it, and the one that matters most is `0x117`, which says which mob on
    // screen is the player's own. Without that the client has a world and
    // nothing to put its camera on, so it keeps the arrival camera up and
    // asks again with `0xF0B` twice a second.
    let mut frames = vec![
        encode_signal(OP_SIGNAL_READY, client_id, time, 1),
        zeroed(OP_ENTER_3A2, client_id, ENTER_3A2_SIZE),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_enter_131(),
        encode_send_to_world(&account, &character, client_id, time, &state.skills),
        zeroed(OP_ENTER_12C, 0, ENTER_12C_SIZE),
    ];

    // Two storage slots and the client index, interleaved exactly as the
    // original interleaves them.
    for slot in ENTER_STORAGE_SLOTS {
        frames.push(encode_refresh_item(inventory::STORAGE, slot, &Item::default(), false));
        frames.push(encode_client_index(client_id));
    }
    frames.push(zeroed(OP_ENTER_94C, 0, ENTER_94C_SIZE));


    let effects = Effects::of(&character, &state.items, &session.buffs, &state.skills);
    let stats = stats::of(&character, &state.items, &effects);
    session.cur_hp = stats.max_hp;
    session.cur_mp = stats.max_mp;
    session.character = Some(character);
    Action::Reply(frames)
}

/// `0xF0B`: the client finished loading the scene and is ready to spawn.
///
/// This is where the character finally gets a position on the map. `0x925`
/// says *who* the character is, not *where*. Without this packet the client
/// enters the world and floats in the middle of nowhere.
fn handle_client_ready(state: &State, session: &mut Session) -> Action {
    let Some(character) = session.character.clone() else {
        warn!("0xF0B before a character was chosen");
        return Action::Ignore;
    };

    // Spawning twice throws the player back to the starting point, and the
    // client resends this packet whenever it thinks something is missing,
    // including when trying to walk. Without this guard it gets stuck in a
    // respawn loop.
    if session.spawned {
        // The client sends this twice a second whatever we do. Rather than
        // drop it, spend it: it is the only regular tick a standing player
        // gives us, and it is what lets a monster that came back appear
        // without the player having to walk — and what carries the blows the
        // world tick left for this player.
        let mut frames = refresh_mob_visibility(state, session);
        frames.extend(collect_blows(state, session));
        frames.extend(drop_spent_buffs(state, session));
        return if frames.is_empty() { Action::Ignore } else { Action::Reply(frames) };
    }

    session.spawned = true;
    state.world.enter(session.client_id, character.clone());

    let skills = known_skills(state, &character);
    let effects = Effects::of(&character, &state.items, &session.buffs, &state.skills);
    let mut frames =
        world_burst(&character, session.client_id, &skills, &state.items, &effects, &state.skills);

    // The city is drawn by the client; the people in it are not. Without this
    // the player arrives in an empty town.
    frames.extend(refresh_npc_visibility(state, session));
    frames.extend(refresh_mob_visibility(state, session));

    // Everyone already standing nearby has to appear on this player's screen,
    // and this player on theirs. Both directions use the same spawn packet.
    let neighbours = state.world.visible_to(session.client_id);

    // Everyone nearby sees an arrival, not somebody who was always there.
    // The original makes the same distinction (`Mob/Player.pas:5185`).
    let mine = encode_spawn_as(
        &character,
        session.client_id,
        SPAWN_TELEPORT_IN,
        stats::BASE_SPEED_MOVE as u32,
    );
    for other in &neighbours {
        if let Some(their_character) = &other.character {
            frames.push(encode_spawn(their_character, other.client_id, stats::BASE_SPEED_MOVE as u32));
        }
        other.send(mine.clone());
        session.visible.insert(other.client_id);
    }

    // And last of all, the companion.
    //
    // Last is the point. The original puts it at the very end of arriving,
    // after `SendCreateMob(SPAWN_NORMAL)` and `SendCreateMob(SPAWN_TELEPORT)`
    // have put the character itself on the field (`Mob/Player.pas:5190`). We
    // had it in the burst that carries the character *record*, which the
    // client receives long before it draws anybody -- so the companion was
    // being stood next to a character that did not exist yet, and simply was
    // not there. Taking the stone off and putting it back worked because by
    // then it did.
    frames.extend(pran_frames(state, session));

    info!(
        character = %character.name,
        x = character.x,
        y = character.y,
        neighbours = neighbours.len(),
        npcs = session.visible_npcs.len(),
        "spawning on the map"
    );
    Action::Reply(frames)
}

/// Everything the server sends once the client reports it has loaded.
///
/// There is no acknowledgement packet in this protocol: the client keeps
/// resending `0xF0B` until it has received enough of this burst to consider
/// itself in the world. Sending only the spawn leaves it asking forever, which
/// also makes movement stutter.
///
/// This is the ordered subset of `SendToWorldSends` (`Mob/Player.pas:4945`)
/// that a character with no guild, pran, mount, friends, buffs or quests
/// receives. Packets whose field layout is not yet known are sent with the
/// right size and a zeroed body, which is what an empty list looks like
/// anyway. Three packets from the original are still missing because their
/// size is unknown: `0x138` (cash inventory), `0x936` and `0x91A` (nation).
fn world_burst(
    character: &Character,
    client_id: u16,
    skills: &[usize],
    items: &ItemList,
    effects: &Effects,
    table: &SkillTable,
) -> Vec<Vec<u8>> {
    let (hp, mp) = vitals(character);
    // What the client walks the body at: forty and whatever a mount adds.
    let speed_move = stats::of(character, items, effects).speed_move;

    let mut frames = vec![
        encode_spawn(character, client_id, speed_move),
        encode_skill_list(client_id, skills),
        encode_skills_level(character, client_id, table),
        encode_signal(OP_CASH, 0, 0, character.gold.min(u32::MAX as u64) as u32),
        zeroed(OP_ACCOUNT_STATUS, client_id, SIGNAL_SIZE),
        zeroed(OP_BUFFS, client_id, BUFFS_SIZE),
        zeroed(OP_ACTIVE_TITLE, client_id, ACTIVE_TITLE_SIZE),
        zeroed(OP_RELICS, FIXED_INDEX, RELICS_SIZE),
        encode_refresh_point(character),
        encode_refresh_status(character, items, effects),
        zeroed(OP_ALL_ATTRIBUTES, client_id, ALL_ATTRIBUTES_SIZE),
        encode_level(character, client_id),
        encode_hp_mp(character, client_id, hp, mp),
    ];

    // The original spawns the player a second time here, after its stats have
    // been recomputed. Same opcode, same recipient, fresher numbers.
    frames.push(encode_spawn(character, client_id, speed_move));
    frames
}

/// Provisional health and mana. The real formulas depend on tables we have not
/// read yet; what matters for now is that neither is zero.
fn vitals(character: &Character) -> (u32, u32) {
    (100 + character.level as u32 * 10, 50 + character.level as u32 * 5)
}

/// A packet of the right size with an empty body, for the ones whose fields
/// are not mapped yet. An empty list is all zeroes in this protocol anyway.
fn zeroed(opcode: u16, sender: u16, total_size: usize) -> Vec<u8> {
    frame::encode(
        &Message { sender, opcode, time: 0, body: vec![0u8; total_size - MIN_FRAME] },
        rand::random(),
    )
}

/// `0x109` `TSendRefreshPoint`: the six attributes plus the two spendable
/// point pools. The original copies the first 12 bytes of `TStatus` straight
/// in, so the order here is the attribute order.
fn encode_refresh_point(character: &Character) -> Vec<u8> {
    let mut body = vec![0u8; REFRESH_POINT_SIZE - MIN_FRAME];
    for (i, value) in character.attributes.iter().enumerate() {
        body[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    // Free status points, which the original sends twice: once as the last
    // attribute and once in its own field.
    let free = character.attributes[5];
    body[12..14].copy_from_slice(&free.to_le_bytes());
    // Skill points. This is the field the skill window actually reads — the
    // `SkillsPoint` word of `TSendRefreshPoint` (`Mob/BaseMob.pas:2591`).
    // Leaving it zero here is why the window showed zero however many the
    // character really had.
    body[14..16].copy_from_slice(&character.skill_points.to_le_bytes());
    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REFRESH_POINT, time: 0, body },
        rand::random(),
    )
}

/// Where each stat sits in the `0x10A` body, from `TSendRefreshStatus`
/// (`Data/Packets.pas:533`). The gaps between them are the record's own
/// `Null1`, `Null2` and `Null3`, which the original never writes.
mod status_offset {
    pub const ATTACK: usize = 0;
    pub const DEFENCE: usize = 2;
    pub const MAGIC_ATTACK: usize = 4;
    pub const MAGIC_DEFENCE: usize = 6;
    pub const SPEED_MOVE: usize = 20;
    pub const CRITICAL: usize = 30;
    pub const DODGE: usize = 34;
    pub const ACCURACY: usize = 36;
    pub const DOUBLE_ATTACK: usize = 38;
    pub const RESISTANCE: usize = 40;
}

/// `0x10A` `TSendRefreshStatus`: the numbers the character sheet shows.
///
/// This is the packet the window opened with C reads, and every field of it
/// but the speed used to go out as zero — so a player in full armour was told
/// they had no attack, no defence and no critical. The values are
/// `GetCurrentScore`'s, worked out in [`stats::of`].
fn encode_refresh_status(character: &Character, items: &ItemList, effects: &Effects) -> Vec<u8> {
    let stats = stats::of(character, items, effects);
    let mut body = vec![0u8; REFRESH_STATUS_SIZE - MIN_FRAME];
    let mut put = |at: usize, value: u32| {
        body[at..at + 2].copy_from_slice(&(value.min(u16::MAX as u32) as u16).to_le_bytes());
    };

    put(status_offset::ATTACK, stats.attack);
    put(status_offset::DEFENCE, stats.defence);
    put(status_offset::MAGIC_ATTACK, stats.magic_attack);
    put(status_offset::MAGIC_DEFENCE, stats.magic_defence);
    // Not the character's stored speed: the original starts from forty and
    // adds what effects say, and never reads the field. A mount is thirty of
    // it, which is the whole reason to be on one.
    put(status_offset::SPEED_MOVE, stats.speed_move);
    put(status_offset::CRITICAL, stats.critical);
    put(status_offset::DODGE, stats.dodge);
    put(status_offset::ACCURACY, stats.accuracy);
    put(status_offset::DOUBLE_ATTACK, stats.double_attack);
    put(status_offset::RESISTANCE, stats.resistance);

    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REFRESH_STATUS, time: 0, body },
        rand::random(),
    )
}

/// `0x108` `TSendCurrentLevel`: level, a constant, and experience.
fn encode_level(character: &Character, client_id: u16) -> Vec<u8> {
    let mut body = vec![0u8; LEVEL_SIZE - MIN_FRAME];
    // Same convention as everywhere else: the client adds 1.
    body[0..2].copy_from_slice(&character.level.saturating_sub(1).to_le_bytes());
    body[2..4].copy_from_slice(&LEVEL_UNK.to_le_bytes());
    body[4..12].copy_from_slice(&character.exp.to_le_bytes());
    frame::encode(
        &Message { sender: client_id, opcode: OP_LEVEL, time: 0, body },
        rand::random(),
    )
}

/// `0x103` `TSendCurrentHPMPPacket`: maximum and current HP and MP.
fn encode_hp_mp(character: &Character, client_id: u16, hp: u32, mp: u32) -> Vec<u8> {
    // `MaxHP, CurHP, MaxMP, CurMP` in that order
    // (`TSendCurrentHPMPPacket`, `Data/Packets.pas:478`). The two arguments are
    // what the character has *now*; the ceilings are worked out here.
    //
    // Writing the current value into both fields is what made the client say
    // there was no mana while the bar still looked full: every spell cast
    // lowered the maximum along with the amount, so after a few the client
    // believed the pool itself had shrunk to nothing.
    let (max_hp, max_mp) = vitals(character);
    let (hp, mp) = (hp.min(max_hp), mp.min(max_mp));

    let mut body = vec![0u8; HP_MP_SIZE - MIN_FRAME];
    body[0..4].copy_from_slice(&max_hp.to_le_bytes());
    body[4..8].copy_from_slice(&hp.to_le_bytes());
    body[8..12].copy_from_slice(&max_mp.to_le_bytes());
    body[12..16].copy_from_slice(&mp.to_le_bytes());
    // The last field marks an update rather than a login; 0 on this path.
    frame::encode(
        &Message { sender: client_id, opcode: OP_HP_MP, time: 0, body },
        rand::random(),
    )
}

/// `0x301`: the player walked.
///
/// The original server **returns nothing to the mover**: the client moves
/// on its own and the server only relays the same packet to whoever can see
/// the player (`SendToVisible(..., false)` in `PacketHandlers.pas:892`).
/// While there is no registry of online players, storing the position does.
fn handle_move(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(movement) = Movement::parse(&message.body) else {
        warn!(size = message.body.len(), "0x301 packet too short");
        return Action::Ignore;
    };

    // The first few movements of a session are dumped in full: the layout of
    // this packet is only confirmed against the real client, and a wrong
    // offset here reads garbage as a coordinate.
    if session.moves_logged < MOVES_TO_DUMP {
        session.moves_logged += 1;
        debug!(
            x = movement.x,
            y = movement.y,
            move_type = movement.move_type,
            speed = movement.speed,
            body = %hex_dump(&message.body),
            "0x301 movement"
        );
    }

    if !movement.is_valid() {
        warn!(x = movement.x, y = movement.y, "invalid destination");
        return Action::Ignore;
    }

    // The original compares the header id against the player's and drops the
    // packet when they disagree. We go further and simply never read it: the
    // movement is applied to whoever owns this connection, so a client cannot
    // move somebody else no matter what it puts in the field.
    if message.sender != session.client_id {
        debug!(
            claimed = message.sender,
            assigned = session.client_id,
            "0x301 header carries another id; moving the connection's own player"
        );
    }

    // Teleport is server-originated. Honouring it from a client packet is
    // exactly the exploit that lets a modified client jump anywhere on the
    // map, so it is refused outright, as the original does.
    if movement.move_type == MOVE_TELEPORT {
        warn!(x = movement.x, y = movement.y, "client asked to teleport itself, refused");
        return Action::Ignore;
    }

    // Beyond walking, the real client sends other move types (16 has been
    // observed in the wild). The original drops those entirely; we still
    // record where the player ended up, because a stale position would be
    // wrong the moment positions get persisted.
    if movement.move_type != MOVE_NORMAL {
        debug!(move_type = movement.move_type, "move type other than walking");
    }

    let (x, y) = (movement.x as u32, movement.y as u32);
    if let Some(character) = session.character.as_mut() {
        character.x = x;
        character.y = y;
        session.dirty = true;
    }
    state.world.move_to(session.client_id, x, y);

    // Standing up. A player who walks is no longer sitting or dancing, so the
    // next person to come into view must not be told that they are — the
    // original clears it at the end of `MovementCommand`.
    session.action = 0;
    state.world.act(session.client_id, 0);

    // The mover hears nothing back: the client moves itself and would fight
    // its own echo. Everyone who can see it gets the same packet, carrying
    // our id so they know who walked.
    let relay = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: OP_MOVE,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(session.client_id, relay);

    refresh_visibility(state, session)
}

/// Spawns and removes players as they walk into and out of range.
///
/// Without this, two players who log in far apart never see each other no
/// matter how close they walk. The set lives on the session so each side
/// appears exactly once, and disappears exactly once.
fn refresh_visibility(state: &State, session: &mut Session) -> Action {
    if session.character.is_none() {
        return Action::Ignore;
    }

    let neighbours = state.world.visible_to(session.client_id);
    let now: HashSet<u16> = neighbours.iter().map(|p| p.client_id).collect();

    let mut frames = refresh_npc_visibility(state, session);
    frames.extend(refresh_mob_visibility(state, session));
    let character = session.character.as_ref().expect("checked above");
    let mine = encode_spawn(character, session.client_id, session.effects(state).plus(crate::effects::id::RUNSPEED) + stats::BASE_SPEED_MOVE as u32);

    for other in &neighbours {
        if session.visible.insert(other.client_id) {
            if let Some(their_character) = &other.character {
                frames.push(encode_spawn(their_character, other.client_id, stats::BASE_SPEED_MOVE as u32));
            }
            other.send(mine.clone());

            // A spawn draws the character standing. Whoever is sitting or
            // dancing needs the action sent after it, or each of the two sees
            // the other on their feet (`SendSpawn`, `Mob/BaseMob.pas:2361`).
            if other.action != 0 {
                frames.push(encode_action(other.client_id, other.action, 1));
            }
            if session.action != 0 {
                other.send(encode_action(session.client_id, session.action, 1));
            }
        }
    }

    // Anyone no longer in range leaves both screens.
    let gone: Vec<u16> = session.visible.difference(&now).copied().collect();
    for id in gone {
        session.visible.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
        if let Some(other) = neighbours.iter().find(|p| p.client_id == id) {
            other.send(encode_remove_mob(session.client_id, DELETE_NORMAL));
        }
    }

    if frames.is_empty() {
        Action::Ignore
    } else {
        Action::Reply(frames)
    }
}

/// Places townspeople on the screen and takes them off it again.
///
/// NPCs are the other half of visibility, and the simpler half: they never
/// move, so only the player walking changes the answer. They are also the
/// first thing a player notices missing — an empty city looks broken in a way
/// an empty field does not.
fn refresh_npc_visibility(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let Some(character) = session.character.as_ref() else {
        return Vec::new();
    };
    let at = (character.x as f32, character.y as f32);

    let mut frames = Vec::new();
    let near: HashSet<u16> = state
        .world
        .npcs_near(at, DISTANCE_TO_WATCH)
        .iter()
        .map(|npc| npc.id)
        .collect();

    for npc in state.world.npcs_near(at, DISTANCE_TO_WATCH) {
        if session.visible_npcs.insert(npc.id) {
            frames.push(encode_npc_spawn(npc));
        }
    }

    // The wider radius is what keeps an NPC from flickering while a player
    // walks back and forth across the edge of the watch distance.
    let gone: Vec<u16> = session
        .visible_npcs
        .iter()
        .copied()
        .filter(|id| !near.contains(id))
        .filter(|id| {
            state
                .world
                .npcs()
                .iter()
                .find(|npc| npc.id == *id)
                .is_none_or(|npc| !within(at, (npc.x, npc.y), DISTANCE_TO_FORGET))
        })
        .collect();

    for id in gone {
        session.visible_npcs.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
    }

    frames
}

/// Puts monsters on the screen and takes them off it again.
///
/// The same two-radius rule as everything else, with one addition: a monster
/// that died while on screen has to be removed even though it has not moved.
fn refresh_mob_visibility(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let Some(character) = session.character.as_ref() else {
        return Vec::new();
    };
    let at = (character.x as f32, character.y as f32);

    let near = state.world.mobs_near(at, DISTANCE_TO_WATCH);
    let mut frames = Vec::new();

    for mob in &near {
        if session.visible_mobs.insert(mob.id) {
            frames.push(encode_mob_spawn(mob));
        }
    }

    // Anything on screen that is no longer near enough, or no longer alive,
    // comes off it. Asking the world for each one covers the second case:
    // a monster killed by somebody else is not in `near` any more.
    let gone: Vec<u16> = session
        .visible_mobs
        .iter()
        .copied()
        .filter(|id| !near.iter().any(|m| m.id == *id))
        .filter(|id| {
            state
                .world
                .mob(*id)
                .is_none_or(|mob| !mob.is_alive() || !within(at, mob.position(), DISTANCE_TO_FORGET))
        })
        .collect();

    for id in gone {
        session.visible_mobs.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
    }

    frames
}

fn within(a: (f32, f32), b: (f32, f32), radius: f32) -> bool {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy <= radius * radius
}

/// The NPC flavour of `0x349` (`TSendCreateNpcPacket`). Same size and mostly
/// the same layout as a player's, with the differences the original makes in
/// `TBaseNpc.GetCreateMob`: the model comes from the NPC's own equipment, the
/// name is an index into the client's string table rather than text, and the
/// job title travels with it.
fn encode_npc_spawn(npc: &Npc) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    // The name field carries the digits of a string index, which is what the
    // file holds and what the client looks up. Writing the real name here
    // would show a number-shaped blank instead.
    if let Some(index) = npc.name_index {
        write_fixed_str(&mut body[off::NAME..off::NAME + 16], &index.to_string());
    }

    for (i, item) in npc.equip.iter().enumerate() {
        put16(&mut body, off::EQUIP + i * 2, *item);
    }

    body[off::POSITION_X..off::POSITION_X + 4].copy_from_slice(&npc.x.to_le_bytes());
    body[off::POSITION_Y..off::POSITION_Y + 4].copy_from_slice(&npc.y.to_le_bytes());
    put32(&mut body, off::ROTATION, npc.rotation as u32);

    // The original sends MaxHP twice, once as the mana ceiling as well.
    put32(&mut body, off::MAX_HP, npc.max_hp);
    put32(&mut body, off::MAX_MP, npc.max_hp);
    put32(&mut body, off::CUR_HP, npc.cur_hp.min(npc.max_hp));
    put32(&mut body, off::CUR_MP, npc.cur_mp.min(npc.max_hp));

    body[off::UNK0] = NPC_UNK0;
    body[off::SPEED_MOVE] = npc.speed_move as u8;
    body[off::SPAWN_TYPE] = SPAWN_NORMAL;
    body[off::SIZES..off::SIZES + 4].copy_from_slice(&npc.sizes);

    body[npc_offset::IS_SERVICE] = NPC_IS_SERVICE;
    put16(&mut body, npc_offset::EFFECT_TYPE, NPC_EFFECT_TYPE);
    write_fixed_str(
        &mut body[npc_offset::TITLE..npc_offset::TITLE + npc_offset::TITLE_MAX],
        &npc.title,
    );

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: npc.id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `0x301` body (`TMovementPacket`, `Data/Packets.pas`): destination as two
/// `f32`, seis bytes de padding, tipo de movimento e velocidade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Movement {
    pub x: f32,
    pub y: f32,
    pub move_type: u8,
    pub speed: u8,
}

impl Movement {
    const MOVE_TYPE: usize = 14;
    const SPEED: usize = 15;
    pub const BODY_SIZE: usize = 20;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            x: f32::from_le_bytes(body[0..4].try_into().ok()?),
            y: f32::from_le_bytes(body[4..8].try_into().ok()?),
            move_type: body[Self::MOVE_TYPE],
            speed: body[Self::SPEED],
        })
    }

    /// Mesma checagem do original (`TPosition.IsValid`): recusa infinito e
    /// NaN. Note that `(0, 0)` passes, as it did there.
    pub fn is_valid(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// `TSendCreateMobPacket` (`0x349`): places a creature on the map.
///
/// The original sends this same packet three times on world entry: twice to
/// the player themselves and once to those nearby. Here we send only the
/// player's own; the neighbour one lands once there is a registry of
/// online players exists.
fn encode_spawn(character: &Character, client_id: u16, speed_move: u32) -> Vec<u8> {
    encode_spawn_as(character, client_id, SPAWN_NORMAL, speed_move)
}

/// The same, saying how the creature is appearing.
fn encode_spawn_as(
    character: &Character,
    client_id: u16,
    spawn_type: u8,
    speed_move: u32,
) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put_f32 = |b: &mut Vec<u8>, at: usize, v: f32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    write_fixed_str(&mut body[off::NAME..off::NAME + 16], &character.name);

    // Equip[0] is the class and Equip[1] the hair, as in the character list.
    put16(&mut body, off::EQUIP, character.class_index);
    put16(&mut body, off::EQUIP + 2, character.hair);

    // And slots 2 to 7 are what the character is actually wearing. Without
    // these the client draws a body and a haircut and nothing else: the
    // armour is in the record it holds, and it does not dress anyone from
    // there — it dresses them from the spawn.
    for slot in WORN_SLOTS {
        put16(&mut body, off::EQUIP + slot as usize * 2, worn_appearance(character, slot));
    }

    // The stone and the mount, by item id rather than by appearance: the
    // original copies `Equip[8].Index` and `Equip[9].Index` straight across.
    // Without the second of these a player on a horse is drawn on foot, since
    // the equip array above stops at slot seven and never reaches it.
    put16(&mut body, off::ITEM_EFF_STONE, worn_index(character, STONE_SLOT));
    put16(&mut body, off::ITEM_EFF_MOUNT, worn_index(character, MOUNT_SLOT));

    body[off::ARMA_REFINE] = SPAWN_ARMA_REFINE;

    // Floating point coordinates: the original server copies the pair of
    // `Single` straight from the account, with no scaling at all.
    put_f32(&mut body, off::POSITION_X, character.x as f32);
    put_f32(&mut body, off::POSITION_Y, character.y as f32);
    put32(&mut body, off::ROTATION, 0);

    let hp = 100 + character.level as u32 * 10;
    let mp = 50 + character.level as u32 * 5;
    put32(&mut body, off::MAX_HP, hp);
    put32(&mut body, off::MAX_MP, mp);
    put32(&mut body, off::CUR_HP, hp);
    put32(&mut body, off::CUR_MP, mp);

    body[off::UNK0] = SPAWN_UNK0;
    // The speed everyone sees the character move at. It is not the stored
    // field either: `GetCurrentScore` builds it from forty plus the effects,
    // so a rider spawns at a rider's speed.
    body[off::SPEED_MOVE] = speed_move.min(u8::MAX as u32) as u8;
    body[off::SPAWN_TYPE] = spawn_type;
    body[off::SIZES..off::SIZES + 4].copy_from_slice(&character.sizes);

    // Without a guild, the field carries only the nation shifted 12 bits.
    put16(&mut body, off::GUILD_AND_NATION, character.nation << 12);
    put16(&mut body, off::EFFECTS + 2, SPAWN_EFFECT_1);

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `TItem` (`Data/MiscData.pas:44`), the twenty bytes an item travels as,
/// in the character record and everywhere else the protocol carries one.
///
/// ```text
/// 0   u16  Index, the id in the item table; zero means an empty slot
/// 2   u16  APP, an appearance that overrides the real look
/// 4   i32  Identific
/// 8   u8[3] effect index, then u8[3] effect value
/// 14  u8   durability now, u8 durability ceiling
/// 16  u16  Refi, the refine level
/// 18  u16  Time, when a rented item expires
/// ```
fn write_item(out: &mut [u8], item: &Item) {
    debug_assert_eq!(out.len(), character_offset::ITEM_SIZE);
    out[0..2].copy_from_slice(&item.index.to_le_bytes());
    out[2..4].copy_from_slice(&item.appearance.to_le_bytes());
    out[4..8].copy_from_slice(&item.identific.to_le_bytes());
    out[8..11].copy_from_slice(&item.effect_index);
    out[11..14].copy_from_slice(&item.effect_value);
    out[14] = item.durability_min;
    out[15] = item.durability_max;
    out[16..18].copy_from_slice(&item.refine.to_le_bytes());
    // Last, because for everything but a mount the expiry overlaps the top
    // byte of the count above and is the one that wins.
    expiry::write_into(out, item);
}


/// `0x30F`: the player clicked an NPC, or picked something from its menu.
///
/// The same packet does both, told apart by the option field: zero is the
/// click and anything else is a choice. The distance is checked on every one
/// of them, not only on the click, because a window left open while walking
/// away would otherwise keep working from across the map.
fn handle_open_npc(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = dialog::OpenNpc::parse(&message.body) else {
        warn!(size = message.body.len(), "0x30F packet too short");
        return Action::Ignore;
    };

    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let at = (character.x as f32, character.y as f32);

    let npc_id = request.npc as u16;
    let Some(npc) = state.world.npcs().iter().find(|n| n.id == npc_id) else {
        debug!(npc = npc_id, "0x30F for an npc that is not in the world");
        return Action::Reply(vec![encode_menu_close()]);
    };

    if !within(at, (npc.x, npc.y), dialog::TALK_RANGE) {
        session.opened_npc = None;
        return Action::Reply(vec![
            encode_client_message(session.client_id, "You are too far away."),
            encode_menu_close(),
        ]);
    }

    match request.option {
        dialog::option::OPEN => {
            session.opened_npc = Some(npc_id);
            let entries = dialog::entries(npc);
            info!(npc = npc_id, name = %npc.label, entries = entries.len(), "npc menu");

            let mut frames = Vec::with_capacity(entries.len() + 2);
            frames.push(encode_menu_begin(session.client_id));
            frames.push(encode_signal(dialog::OP_MENU_OWNER, session.client_id, 0, npc_id as u32));
            frames.extend(entries.into_iter().map(encode_menu_entry));
            Action::Reply(frames)
        }
        dialog::option::SHOP => {
            if !npc.sells() {
                return Action::Reply(vec![
                    encode_client_message(session.client_id, "There is nothing for sale."),
                    encode_menu_close(),
                ]);
            }
            session.opened_npc = Some(npc_id);
            info!(npc = npc_id, name = %npc.label, "shop opened");

            // The conversation closes before the shop opens. The client
            // letterboxes the screen while a menu is up, and leaving it up
            // behind the shop is what looks like a cutscene that will not
            // end. The original closes it for every option except talking,
            // quests and the menu heading (`PacketHandlers.pas:3382`).
            Action::Reply(vec![encode_menu_close(), encode_show_shop(session.client_id, npc)])
        }
        dialog::option::SKILLS => {
            // The skill master's window is the same packet as the bar, with
            // the NPC named and a send type that tells the client to open the
            // trainer rather than redraw the bar
            // (`TNPCHandlers.ShowSkills` -> `SendPlayerSkills(NpcId)`).
            let Some(character) = session.character.as_ref() else {
                return Action::Ignore;
            };
            let skills = known_skills(state, character);
            info!(npc = npc_id, name = %npc.label, skills = skills.len(), "skill trainer");
            Action::Reply(vec![
                encode_menu_close(),
                encode_skill_list_from(session.client_id, npc_id, &skills),
            ])
        }
        dialog::option::QUESTS => {
            // A companion standing at its own wall is the more specific
            // thing to be asking for, so it is answered first.
            if let Some(frames) = evolve_the_pran(state, session) {
                return Action::Reply(frames);
            }

            // The promotion chain was quests, and this is standing in for it
            // until there are quests: an NPC that offers them promotes a
            // character waiting at its tier's wall. See `crate::promotion`.
            let (Some(character), Some(_)) = (session.character.as_ref(), session.account.as_ref())
            else {
                return Action::Ignore;
            };
            match promotion::Promotion::offered(character.tier, character.level) {
                Ok(next) => {
                    session.character.as_mut().expect("checked above").tier = next.tier;
                    session.dirty = true;
                    let character = session.character.as_ref().expect("checked above");
                    let account = session.account.as_ref().expect("checked above");
                    info!(
                        character = %character.name,
                        npc = npc_id,
                        tier = next.tier,
                        cap = next.level_cap,
                        "promoted"
                    );
                    let text = format!("You may now reach level {}.", next.level_cap);
                    // The class name is part of the character record, so the
                    // client only repaints it when the whole record arrives.
                    let refreshed = encode_send_to_world(
                        account,
                        character,
                        session.client_id,
                        PACKET_TIME,
                        &state.skills,
                    );
                    Action::Reply(vec![
                        encode_menu_close(),
                        refreshed,
                        encode_client_message(session.client_id, &text),
                    ])
                }
                Err(refusal) => Action::Reply(vec![
                    encode_menu_close(),
                    encode_client_message(session.client_id, &refusal.message()),
                ]),
            }
        }
        dialog::option::STORAGE | dialog::option::PRAN_STATION => {
            // One window, two sides. `OpenNPC` answers both with
            // `SendStorage` and only the type differs: one draws the chest
            // and the other the pran centre, which is the same eighty-six
            // slots seen from the other end. The two the pran centre cares
            // about are 84 and 85, and they are the two the chest packet
            // does not carry -- `SendStorage` sends them on their own,
            // every time, which is why a pran in one of them was invisible.
            let for_prans = request.option == dialog::option::PRAN_STATION;
            session.opened_option = request.option;
            session.opened_npc = Some(npc_id);
            let Some(account) = session.account.as_ref() else {
                return Action::Ignore;
            };
            info!(npc = npc_id, name = %npc.label, for_prans, "chest opened");

            let mut frames = vec![encode_menu_close()];
            frames.extend(items::open_storage(
                session.client_id,
                account.storage_gold,
                &account.storage,
                if for_prans { STORAGE_TYPE_PRANS } else { STORAGE_TYPE_PLAYER },
            ));

            // What the window is about, for the window that is about it.
            //
            // Ours, and the second half of the same hole: `0x907` is the only
            // packet that carries a pran's name, and the original sends it
            // when one is summoned and at no other time. The station draws a
            // pran the client may know nothing about, and a pran it thinks is
            // unnamed is one it asks the player to name -- before there is any
            // refusal to correct it with.
            //
            // The stone in the first pran slot names which one. Sent only for
            // the pran side of the window, so the plain chest is untouched.
            if for_prans {
                let named_by_the_first_slot = account
                    .storage
                    .get(inventory::STORAGE, inventory::STORAGE_PRAN_SLOTS[0])
                    .and_then(|stone| account.prans.iter().find(|p| p.belongs_to(stone)));
                if let Some(pran) = named_by_the_first_slot {
                    frames.push(frame::encode(
                        &Message {
                            sender: dialog::FIXED_INDEX,
                            opcode: pran::OP_WORLD,
                            time: 0,
                            body: pran::world_body(pran),
                        },
                        rand::random(),
                    ));
                }
            }
            Action::Reply(frames)
        }
        dialog::option::CLOSE => {
            session.opened_npc = None;
            Action::Reply(vec![encode_menu_close()])
        }
        other => {
            // Everything else is a system we have not built. Saying so beats
            // a window that opens onto nothing and never closes.
            let name = dialog::option_text(other as u8);
            debug!(npc = npc_id, option = other, text = name, "npc option not implemented");

            // Closed first, for the same reason the shop closes it: the
            // client keeps the screen letterboxed while a menu is up.
            let mut frames = Vec::new();
            if !request.keeps_window_open() {
                frames.push(encode_menu_close());
            }
            frames.push(encode_client_message(
                session.client_id,
                &format!("{name} is not available yet."),
            ));
            Action::Reply(frames)
        }
    }
}

/// `0x348`: the client closed the window on its own.
fn handle_close_npc(session: &mut Session) -> Action {
    session.opened_npc = None;
    Action::Ignore
}

/// `0x313`: buy from the shop that is open.
fn handle_buy(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = shop::Buy::parse(&message.body) else {
        warn!(size = message.body.len(), "0x313 packet too short");
        return Action::Ignore;
    };

    let Some(npc) = open_shop(state, session, request.npc as u16).cloned() else {
        return Action::Reply(vec![encode_client_message(
            session.client_id,
            &shop::ShopError::WrongNpc.message(),
        )]);
    };

    let client_id = session.client_id;
    let chest = chest_gold(session);
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let outcome = shop::buy(&npc, request, &mut character.items, character.gold, &state.items);

    match outcome {
        Ok(change) => {
            character.gold = change.gold;
            session.dirty = true;
            info!(
                npc = npc.id,
                item = change.item.index,
                slot = change.slot,
                gold = change.gold,
                "bought"
            );
            let mut frames = Vec::new();
            // Whatever currency was spent goes back first, so the client
            // redraws the drained stacks before it draws the new item.
            for (slot, left) in &change.spent {
                frames.push(encode_refresh_item(inventory::BAG, *slot, left, false));
            }
            frames.push(encode_refresh_item(inventory::BAG, change.slot, &change.item, true));
            frames.push(encode_refresh_money(change.gold, chest));
            Action::Reply(frames)
        }
        Err(e) => {
            debug!(npc = npc.id, error = %e, "purchase refused");
            Action::Reply(vec![encode_client_message(client_id, &e.message())])
        }
    }
}

/// `0x314`: sell a bag slot to the shop that is open.
fn handle_sell(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = shop::Sell::parse(&message.body) else {
        warn!(size = message.body.len(), "0x314 packet too short");
        return Action::Ignore;
    };

    if open_shop(state, session, request.npc as u16).is_none() {
        return Action::Reply(vec![encode_client_message(
            session.client_id,
            &shop::ShopError::WrongNpc.message(),
        )]);
    }

    let client_id = session.client_id;
    let chest = chest_gold(session);
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let outcome = shop::sell(request, &mut character.items, character.gold, &state.items);

    match outcome {
        Ok(change) => {
            character.gold = change.gold;
            session.dirty = true;
            info!(slot = change.slot, gold = change.gold, "sold");
            Action::Reply(vec![
                encode_refresh_item(inventory::BAG, change.slot, &change.item, true),
                encode_refresh_money(change.gold, chest),
            ])
        }
        Err(e) => {
            debug!(error = %e, "sale refused");
            Action::Reply(vec![encode_client_message(client_id, &e.message())])
        }
    }
}

/// The NPC the client says it is trading with, if it may.
///
/// The gate is distance, not a flag saying the window is open. Two reasons.
/// The flag was wrong: the client closes the option menu the moment the shop
/// window replaces it, and pressing escape closes it too, so a real player
/// buying normally would be refused. And the flag was never the security
/// property — standing next to the NPC is. A client that sends a purchase
/// without opening a window first has done nothing it could not have done by
/// clicking, since the stock and the prices are ours.
///
/// The original checks a flag it only ever sets when the player picks *talk*
/// or *quest*, which means buying from a merchant with neither of those on
/// its menu — Roze, for one — could never have worked there either.
fn open_shop<'a>(state: &'a State, session: &Session, claimed: u16) -> Option<&'a Npc> {
    let npc = state.world.npcs().iter().find(|n| n.id == claimed)?;

    let character = session.character.as_ref()?;
    let at = (character.x as f32, character.y as f32);
    if !within(at, (npc.x, npc.y), dialog::TALK_RANGE) {
        debug!(claimed, "shop packet from too far away");
        return None;
    }
    Some(npc)
}

/// Slots whose contents are drawn on the character. Changing one of them is
/// the difference between `ReSpawn` and `UpdatePoint` in the original: both
/// recompute the numbers, but only these make everyone redraw the player.
const WORN_ON_THE_BODY: std::ops::RangeInclusive<u16> = 2..=9;

/// `0x31C`: learn or rank up a skill at a trainer (`LearnSkill`).
///
/// The checks are the original's, in order: the skill has to exist and belong
/// to the class, the character has to be high enough level, have the points it
/// costs and the gold it costs. Learning bumps the skill's level in the record
/// — a basic stays marked as it was, an advanced skill climbs a rank — and
/// spends the points and the gold. The client is then sent the fresh skill
/// list, the fresh levels, and the new purse.
///
/// # The rank has to reach the bar or it is worth nothing
///
/// A rank is not a modifier on a skill: it is a *different id*, one past the
/// last (`Index + Level - 1`, which is how `SetPlayerSkills` reads the record
/// back). The client casts whatever id its bar holds, so a rank bought and
/// never written onto the bar is a rank the player paid for and will never
/// cast — every spell keeps hitting for the first rank's damage for the rest
/// of the character's life. The original closes this with the last line of
/// `LearnSkill`, `UpdateAllOnBar(SkillIndex - 1, SkillIndex)`
/// (`PacketHandlers.pas:7738`).
fn handle_learn_skill(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 8 {
        warn!(size = message.body.len(), "0x31C packet too short");
        return Action::Ignore;
    }
    let skill_id = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as usize;
    // Which trainer the request came from. The reply has to name it, or the
    // trainer window will not redraw and the player clicks a second time.
    let npc = u32::from_le_bytes(message.body[4..8].try_into().unwrap()) as u16;

    let client_id = session.client_id;
    let chest = chest_gold(session);

    // `if (Player.SkillUpgraded = Packet.SkillIndex) then Exit`. The trainer
    // window sends the same id again when it has not redrawn yet, and without
    // this a double click buys the rank twice.
    if session.skill_upgraded == Some(skill_id) {
        debug!(skill = skill_id, "0x31C for the rank just bought");
        return Action::Ignore;
    }

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let class = character.class_number() as u32;

    let refuse = |text: &str| Action::Reply(vec![encode_client_message(client_id, text)]);

    let Some(skill) = state.skills.get(skill_id) else {
        return refuse("Esta habilidade não está disponível.");
    };
    if !ability::belongs_to(class, skill_id) {
        return refuse("Esta habilidade não pertence a sua classe.");
    }
    if skill.min_level() > character.level as u32 {
        return refuse("Não possui level necessário.");
    }
    let cost = skill.skill_points() as u16;
    if cost > character.skill_points {
        return refuse("Não possui pontos de habilidade necessário.");
    }
    let learn_cost = skill.learn_cost() as u64;
    if learn_cost > character.gold {
        return refuse("Não possui gold suficiente.");
    }
    let Some(slot) = ability::record_slot(class, skill_id) else {
        return refuse("Esta habilidade não está disponível.");
    };

    // Advanced skills climb a rank in the record; a basic is already marked
    // learned and stays so. Either way the points and the gold are spent.
    if slot >= BASIC_SKILL_COUNT {
        character.skill_list[slot] = character.skill_list[slot].saturating_add(1);
    }
    spend_skill_point(character, cost);
    character.gold -= learn_cost;

    // The bar follows the rank. The slot holds the id of the rank cast until
    // now, which is one below the one just bought, and the client goes on
    // casting whatever is written there.
    let bar_slot = ability::slot_on_bar(&character.item_bar, skill_id.saturating_sub(1));
    if let Some(at) = bar_slot {
        character.item_bar[at] = ability::on_bar(skill_id);
    }

    session.dirty = true;
    session.skill_upgraded = Some(skill_id);
    info!(skill = skill_id, slot, cost, bar = ?bar_slot, "skill learned");

    let character = session.character.as_ref().expect("checked above");
    let known = known_skills(state, character);
    let mut frames = vec![
        // The skill list said to come from the trainer, so its window redraws
        // the newly learned skill; `SendPlayerSkills(NPCIndex)` in the original.
        encode_skill_list_from(client_id, npc, &known),
        encode_skills_level(character, client_id, &state.skills),
        encode_refresh_point(character),
        encode_refresh_money(character.gold, chest),
    ];
    if let Some(at) = bar_slot {
        frames.push(encode_item_bar_slot(at, ITEM_BAR_SKILL, skill_id as u32));
    }
    Action::Reply(frames)
}

/// What a skill costs in points, spent the way the original spends it.
///
/// The last point is a special case in `LearnSkill` and not a kind one: rather
/// than subtracting the cost it sets the skill points to nought **and the
/// unspent status points with them** (`PacketHandlers.pas:7723`). It reads
/// like a slip — `Status` is the pool `0x213` spends and nothing else in the
/// function touches it — but it is what the server our client talked to did,
/// so it is what happens here. It is also the whole reason a character should
/// spend status points as they come rather than banking them.
fn spend_skill_point(character: &mut Character, cost: u16) {
    if character.skill_points == 1 {
        character.skill_points = 0;
        character.attributes[FREE_POINTS] = 0;
    } else {
        character.skill_points -= cost;
    }
}

/// What starting over costs: half a thousand gold a level
/// (`Taxa := (Level * 1000) div 2`).
fn reset_fee(level: u16) -> u64 {
    level as u64 * 1000 / 2
}

/// The sheet a character goes back to.
///
/// `ResetSkills` does it in two steps that read oddly together: it copies the
/// whole of the class template's skills back over the character's, and then
/// clears every advanced level it just copied and hands back the first one.
/// So the six basic skills come from the template and the forty advanced ones
/// do not — whatever the template had learned past the first is gone.
fn starting_skills(template: Option<&aika_data::template::Template>) -> [u16; 60] {
    let mut list = match template {
        Some(template) => creation::skill_list_from(template),
        // Without a template there is nothing to copy, and the six basics are
        // what a character cannot be without: unmarked, the client refuses to
        // cast at all. Same fallback as the one on the way into the world.
        None => {
            let mut list = [0u16; 60];
            list[..BASIC_SKILL_COUNT].fill(BASIC_SKILL_LEARNED);
            list
        }
    };
    list[BASIC_SKILL_COUNT..].fill(0);
    list[BASIC_SKILL_COUNT] = 1;
    list
}

/// `0x32A`: forget every skill and take the points back (`ResetSkills`,
/// `PacketHandlers.pas:7744`).
///
/// The fee is the only thing that can refuse it. After that the bar is wiped
/// slot by slot — a bar left pointing at ranks the character no longer has
/// would cast them, since the id in the slot is the whole of what the client
/// sends — the sheet goes back to the class template with the first advanced
/// skill at rank one, and the points come back as the level's whole
/// entitlement rather than as a count of what was spent.
///
/// The reply is the original's list of sends, repeats included: it says the
/// skills, the score, the points and the vitals twice each, in that order.
/// They are left in because the order packets go out in has already cost this
/// project a week once, and the cost of sending one twice is a few bytes.
///
/// Passives are the one line not carried over. `SearchSkillsPassive(1)` takes
/// off what the learned passives were adding, and there are none here to take
/// off yet.
fn handle_reset_skills(state: &State, session: &mut Session) -> Action {
    let client_id = session.client_id;
    let chest = chest_gold(session);
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    let fee = reset_fee(character.level);
    if character.gold < fee {
        return Action::Reply(vec![encode_client_message(
            client_id,
            "Você não possui gold sulficiente para reniciar as suas habilidades!",
        )]);
    }

    // The bar first, and every slot of it whether or not it held anything:
    // the original sends forty of these and the client redraws each.
    character.item_bar = [0; 40];
    let mut frames: Vec<Vec<u8>> = (0..character.item_bar.len())
        .map(|slot| encode_item_bar_slot(slot, 0, 0))
        .collect();

    let class = character.class_number();
    character.skill_list = starting_skills(state.template(class));
    character.gold -= fee;
    character.skill_points = crate::store::skill_points_for(character.level);
    session.dirty = true;
    info!(fee, points = character.skill_points, "skills reset");

    let effects = session.effects(state);
    let character = session.character.as_ref().expect("checked above");
    let known = known_skills(state, character);
    let (max_hp, max_mp) = vitals(character);
    session.cur_hp = session.cur_hp.min(max_hp);
    session.cur_mp = session.cur_mp.min(max_mp);

    frames.extend([
        encode_hp_mp(character, client_id, session.cur_hp, session.cur_mp),
        encode_skill_list(client_id, &known),
        encode_refresh_point(character),
        encode_refresh_status(character, &state.items, &effects),
        encode_level(character, client_id),
        encode_hp_mp(character, client_id, session.cur_hp, session.cur_mp),
        encode_skill_list(client_id, &known),
        encode_refresh_money(character.gold, chest),
        encode_skills_level(character, client_id, &state.skills),
        encode_refresh_status(character, &state.items, &effects),
        encode_refresh_point(character),
    ]);

    // `Sair(Player)`: whatever window this came from is closed.
    session.opened_npc = None;
    session.opened_option = 0;
    Action::Reply(frames)
}

/// `TMoveItemPacket` (`0x70F`), 8 bytes of body.
///
/// Four `WORD`s, and **the destination comes first**: `DestType, DestSlot,
/// SrcType, SrcSlot` (`Data/Packets.pas:903`). Reading them the other way
/// round is a bug that looks like the client dragging from empty slots,
/// because every drag arrives backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveItem {
    pub to_container: u16,
    pub to_slot: u16,
    pub from_container: u16,
    pub from_slot: u16,
}

impl MoveItem {
    pub const BODY_SIZE: usize = 8;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            to_container: u16::from_le_bytes(body[0..2].try_into().ok()?),
            to_slot: u16::from_le_bytes(body[2..4].try_into().ok()?),
            from_container: u16::from_le_bytes(body[4..6].try_into().ok()?),
            from_slot: u16::from_le_bytes(body[6..8].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.to_container.to_le_bytes());
        body.extend_from_slice(&self.to_slot.to_le_bytes());
        body.extend_from_slice(&self.from_container.to_le_bytes());
        body.extend_from_slice(&self.from_slot.to_le_bytes());
        body
    }

    pub fn from(&self) -> (u8, u16) {
        (self.from_container as u8, self.from_slot)
    }

    pub fn to(&self) -> (u8, u16) {
        (self.to_container as u8, self.to_slot)
    }
}

/// `TDeleteItemPacket` (`0x32C`), 8 bytes: the slot, then the container.
/// Note the order — it is the opposite of everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteItem {
    pub slot: u32,
    pub container: u32,
}

impl DeleteItem {
    pub const BODY_SIZE: usize = 8;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            slot: u32::from_le_bytes(body[0..4].try_into().ok()?),
            container: u32::from_le_bytes(body[4..8].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.slot.to_le_bytes());
        body.extend_from_slice(&self.container.to_le_bytes());
        body
    }
}

/// `TUseItemPacket` (`0x31D`), 12 bytes: the container, the slot, and a
/// third field whose meaning depends on the kind of item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseItem {
    pub container: u32,
    pub slot: u32,
    pub argument: u32,
}

impl UseItem {
    pub const BODY_SIZE: usize = 12;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            container: u32::from_le_bytes(body[0..4].try_into().ok()?),
            slot: u32::from_le_bytes(body[4..8].try_into().ok()?),
            argument: u32::from_le_bytes(body[8..12].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.container.to_le_bytes());
        body.extend_from_slice(&self.slot.to_le_bytes());
        body.extend_from_slice(&self.argument.to_le_bytes());
        body
    }
}


/// A bare header, which is all a signal is (`ServerSocket.pas:3168`).
fn encode_bare_signal(opcode: u16, index: u16) -> Vec<u8> {
    frame::encode(&Message { sender: index, opcode, time: 0, body: Vec::new() }, rand::random())
}

fn encode_menu_begin(client_id: u16) -> Vec<u8> {
    encode_bare_signal(dialog::OP_MENU_BEGIN, client_id)
}

/// Closing is addressed to the fixed index, not to the player: that is what
/// the original sends (`PacketHandlers.pas:3385`).
fn encode_menu_close() -> Vec<u8> {
    encode_bare_signal(dialog::OP_MENU_CLOSE, dialog::FIXED_INDEX)
}

/// `TShowOptionsPacket` (`0x112`): one line of the menu.
fn encode_menu_entry(option: u8) -> Vec<u8> {
    let body = dialog::menu_entry_body(option);
    debug_assert_eq!(body.len() + MIN_FRAME, dialog::MENU_ENTRY_SIZE);
    frame::encode(
        &Message { sender: MENU_ENTRY_INDEX, opcode: dialog::OP_MENU_ENTRY, time: 0, body },
        rand::random(),
    )
}

/// `TShowShopPacket` (`0x106`): the forty ids the window shows.
fn encode_show_shop(client_id: u16, npc: &Npc) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::SHOW_SHOP_SIZE - MIN_FRAME);
    body.extend_from_slice(&npc.id.to_le_bytes());
    body.extend_from_slice(&shop::SHOP_DEF_BYTE.to_le_bytes());
    for id in shop::stock(npc) {
        body.extend_from_slice(&id.to_le_bytes());
    }

    debug_assert_eq!(body.len() + MIN_FRAME, shop::SHOW_SHOP_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: shop::OP_SHOW_SHOP, time: 0, body },
        rand::random(),
    )
}

/// `TRefreshItemPacket` (`0xF0E`): one slot changed. An empty item clears it.
fn encode_refresh_item(container: u8, slot: u16, item: &Item, notice: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::REFRESH_ITEM_SIZE - MIN_FRAME);
    body.push(notice as u8);
    body.push(container);
    body.extend_from_slice(&slot.to_le_bytes());

    let at = body.len();
    body.resize(at + character_offset::ITEM_SIZE, 0);
    write_item(&mut body[at..], item);

    debug_assert_eq!(body.len() + MIN_FRAME, shop::REFRESH_ITEM_SIZE);
    frame::encode(
        &Message { sender: dialog::FIXED_INDEX, opcode: shop::OP_REFRESH_ITEM, time: 0, body },
        rand::random(),
    )
}

/// `TRefreshMoneyPacket` (`0x312`): what is in the purse and what is in the
/// chest, which the original sends together because the window that shows one
/// shows the other (`RefreshMoney`, `Mob/Player.pas:4374`).
fn encode_refresh_money(gold: u64, chest_gold: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::REFRESH_MONEY_SIZE - MIN_FRAME);
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&gold.to_le_bytes());
    body.extend_from_slice(&chest_gold.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, shop::REFRESH_MONEY_SIZE);
    frame::encode(
        &Message { sender: dialog::FIXED_INDEX, opcode: shop::OP_REFRESH_MONEY, time: 0, body },
        rand::random(),
    )
}

/// `TClientMessagePacket` (`0x984`): a line of text for the player.
fn encode_client_message(client_id: u16, text: &str) -> Vec<u8> {
    let mut body = vec![0u8; CLIENT_MESSAGE_SIZE - MIN_FRAME];
    body[1] = MESSAGE_NOTICE;
    write_fixed_str(&mut body[4..4 + CLIENT_MESSAGE_TEXT], text);

    debug_assert_eq!(body.len() + MIN_FRAME, CLIENT_MESSAGE_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CLIENT_MESSAGE, time: 0, body },
        rand::random(),
    )
}


/// `0x305`: the player turned.
///
/// Relayed rather than answered: the client that sent it already turned, and
/// everyone who can see the player needs to see the same. The value is kept
/// on the presence so somebody walking into view is drawn facing the right
/// way, and it is dropped on logout because the client sends it again the
/// moment the mouse moves.
fn handle_rotate(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0x305 packet too short");
        return Action::Ignore;
    }
    let rotation = u32::from_le_bytes(message.body[0..4].try_into().unwrap());

    if session.rotation == rotation {
        return Action::Ignore;
    }
    session.rotation = rotation;
    state.world.turn(session.client_id, rotation);

    let relay = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: OP_ROTATE,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(session.client_id, relay);
    Action::Ignore
}

/// `0x16E` (`SendRefreshBuffs`): everything currently working on the player.
///
/// Forty slots of skill id and forty of the unix second each one ends at. The
/// original packs them from the front and leaves the rest zero, which is what
/// tells the client the row is empty.
fn encode_buffs(client_id: u16, buffs: &crate::buffs::Buffs, skills: &SkillTable) -> Vec<u8> {
    let now = std::time::SystemTime::now();
    let mut body = vec![0u8; BUFFS_SIZE - MIN_FRAME];
    for (i, (skill, ends_at)) in buffs.running(skills).into_iter().enumerate() {
        body[i * 2..i * 2 + 2].copy_from_slice(&(skill as u16).to_le_bytes());
        let at = BUFFS_TIMES_AT + i * 4;
        let left = crate::buffs::remaining(ends_at, now);
        body[at..at + 4].copy_from_slice(&left.to_le_bytes());
    }

    debug_assert_eq!(body.len() + MIN_FRAME, BUFFS_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_BUFFS, time: 0, body },
        rand::random(),
    )
}

/// `0x16F` (`SendAddBuff`): one buff just started, and when it ends.
fn encode_add_buff(
    client_id: u16,
    skill: usize,
    ends_at: Option<std::time::SystemTime>,
) -> Vec<u8> {
    let mut body = vec![0u8; ADD_BUFF_SIZE - MIN_FRAME];
    body[0..4].copy_from_slice(&(skill as u32).to_le_bytes());
    let left = crate::buffs::remaining(ends_at, std::time::SystemTime::now());
    body[4..8].copy_from_slice(&left.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, ADD_BUFF_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_ADD_BUFF, time: 0, body },
        rand::random(),
    )
}

/// Starts a buff and tells the client everything that changes because of it.
///
/// `SendAddBuff` sends the one buff, then the whole list, then the points and
/// the status — the last two because a buff is one of the things
/// `GetCurrentScore` adds up, so the character sheet has to be redrawn.
fn grant_buff(state: &State, session: &mut Session, skill: usize) -> Vec<Vec<u8>> {
    let now = std::time::SystemTime::now();
    if !session.buffs.add(&state.skills, skill, now) {
        debug!(skill, "that skill does not last, so it is not a buff");
        return Vec::new();
    }
    let client_id = session.client_id;
    // What the list says about it, which is `None` for one that does not run
    // out. A buff that is not in the list at all cannot happen here, since it
    // was just added, but a missing skill would say "already over".
    let ends_at = session
        .buffs
        .running(&state.skills)
        .into_iter()
        .find(|(id, _)| *id == skill)
        .map(|(_, at)| at)
        .unwrap_or(Some(now));

    let mut frames = vec![
        encode_add_buff(client_id, skill, ends_at),
        encode_buffs(client_id, &session.buffs, &state.skills),
    ];
    session.buffs_sent_at = Some(std::time::Instant::now());
    let effects = session.effects(state);
    if let Some(character) = session.character.as_ref() {
        frames.push(encode_refresh_point(character));
        frames.push(encode_refresh_status(character, &state.items, &effects));
    }
    info!(skill, "buff started");
    frames
}

/// `0x21B`: use an item whose whole job is to start a buff (`UseBuffItem`).
///
/// A saddle comes in here rather than through the ordinary use-item packet,
/// which is why using one did nothing at all before: the opcode was not
/// handled and the client had no reply to wait for. The item names a skill in
/// its `UseEffect`, and that skill is the buff.
fn handle_use_buff_item(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0x21B packet too short");
        return Action::Ignore;
    }
    let slot = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as u16;

    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let Some(item) = character.items.get(inventory::BAG, slot).cloned() else {
        debug!(slot, "0x21B on an empty slot");
        return Action::Ignore;
    };
    let Some(def) = state.items.get(item.index as usize) else {
        debug!(item = item.index, "0x21B on an item that is not in the table");
        return Action::Ignore;
    };
    if !matches!(def.item_type(), ITEM_TYPE_BUFF | ITEM_TYPE_BUFF2) {
        debug!(item = item.index, item_type = def.item_type(), "0x21B on something else");
        return Action::Ignore;
    }

    // The saddle is not spent: the original starts the buff and leaves the
    // item where it is, which is what makes it good for thirty days.
    let frames = grant_buff(state, session, def.use_effect() as usize);
    if frames.is_empty() {
        return Action::Ignore;
    }
    Action::Reply(frames)
}

/// `0x218`: one of the two skills a mount carries (`UseMountSkill`).
///
/// The original refuses unless the player is both mounted and has a mount
/// equipped, and turns the byte it is sent into one of two fixed skill ids.
fn handle_mount_skill(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < MOUNT_SKILL_SIZE - MIN_FRAME {
        warn!(size = message.body.len(), "0x218 packet too short");
        return Action::Ignore;
    }
    let which = message.body[0] as usize;
    let client_id = session.client_id;

    let mounted = session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED);
    let has_mount = session
        .character
        .as_ref()
        .and_then(|c| c.items.get(inventory::EQUIP, MOUNT_SLOT))
        .is_some();
    if !mounted || !has_mount {
        return Action::Reply(vec![encode_client_message(
            client_id,
            "O uso dessa habilidade requer estar montado ou com uma montaria equipada",
        )]);
    }

    let Some(&skill) = MOUNT_SKILLS.get(which) else {
        // The original says as much and stops, rather than casting something.
        debug!(which, "a mount has two skills and that is not one of them");
        return Action::Reply(vec![encode_client_message(client_id, "Usando skill de montaria")]);
    };

    // The original builds a `0x320` and hands it to `UseSkill`, so a mount
    // skill goes through the same casting as any other.
    let cast = Message {
        sender: client_id,
        opcode: ability::OP_USE_SKILL,
        time: message.time,
        body: ability::UseSkill { skill: skill as u32, target: client_id as u32, at: (0.0, 0.0) }
            .to_body(),
    };
    cast_skill(state, session, &cast, ability::Named::ByTheServer)
}

/// Where each attribute sits in `character.attributes`, in the order the
/// client numbers them (`GetStatusPoint`: 0 strength, 1 agility, 2 intellect,
/// 3 constitution, 4 luck). The fifth slot is the unspent count, which is not
/// something to spend points on.
const ATTRIBUTE_COUNT: u32 = 5;
const FREE_POINTS: usize = 5;

/// `0x329`: the player clicked a buff off (`RemoveBuff`).
///
/// The body names the skill that started it. Without this a buff that does
/// not run out on its own -- a mount's -- can never be got rid of, and the
/// player stays mounted with nothing to ride.
///
/// The original answers with the fresh list, the vitals, the sheet and the
/// points, because taking an effect off changes all four.
fn handle_remove_buff(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0x329 packet too short");
        return Action::Ignore;
    }
    let skill = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as usize;
    if !session.buffs.remove(skill) {
        debug!(skill, "0x329 for a buff that is not running");
        return Action::Ignore;
    }
    info!(skill, "buff taken off");

    let client_id = session.client_id;
    let effects = session.effects(state);
    let mut frames = vec![encode_buffs(client_id, &session.buffs, &state.skills)];
    if let Some(character) = session.character.as_ref() {
        let (max_hp, max_mp) = vitals(character);
        session.cur_hp = session.cur_hp.min(max_hp);
        session.cur_mp = session.cur_mp.min(max_mp);
        let character = session.character.as_ref().expect("checked above");
        frames.push(encode_hp_mp(character, client_id, session.cur_hp, session.cur_mp));
        frames.push(encode_refresh_status(character, &state.items, &effects));
        frames.push(encode_refresh_point(character));
    }
    Action::Reply(frames)
}

/// `0x213`: spend free points on an attribute (`GetStatusPoint`).
///
/// The original's checks are short and all of them matter: you cannot spend
/// more than you have, and the index has to name one of the five. It then
/// sends the score, the sheet, the points and the vitals, because raising
/// constitution changes how much health there is.
fn handle_status_point(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 8 {
        warn!(size = message.body.len(), "0x213 packet too short");
        return Action::Ignore;
    }
    let which = u32::from_le_bytes(message.body[0..4].try_into().unwrap());
    let amount = u32::from_le_bytes(message.body[4..8].try_into().unwrap());

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    // Nought is not "spend nothing", it is a client that lost count.
    if amount == 0 || which >= ATTRIBUTE_COUNT {
        debug!(which, amount, "0x213 for an attribute that does not exist");
        return Action::Ignore;
    }
    if amount > character.attributes[FREE_POINTS] as u32 {
        debug!(
            amount,
            free = character.attributes[FREE_POINTS],
            "0x213 for more points than the character has"
        );
        return Action::Ignore;
    }

    let amount = amount as u16;
    character.attributes[which as usize] += amount;
    character.attributes[FREE_POINTS] -= amount;
    session.dirty = true;
    info!(which, amount, left = character.attributes[FREE_POINTS], "status points spent");

    // Health and mana move with constitution, so they are recomputed before
    // being sent, exactly as `SendCurrentHPMP` after `GetCurrentScore` does.
    let effects = session.effects(state);
    let character = session.character.as_ref().expect("checked above");
    let (max_hp, max_mp) = vitals(character);
    session.cur_hp = session.cur_hp.min(max_hp);
    session.cur_mp = session.cur_mp.min(max_mp);

    Action::Reply(vec![
        encode_refresh_status(character, &state.items, &effects),
        encode_refresh_point(character),
        encode_hp_mp(character, client_id, session.cur_hp, session.cur_mp),
    ])
}

/// `0x202`: what time is it on the server (`RequestServerTime`).
///
/// The whole of the original is one line: it answers `DateTimeToStr(Now)` as
/// an ordinary client message, the yellow line across the top of the screen.
/// The format is the one Delphi prints under the locale the server runs in,
/// `dd/mm/yyyy hh:nn:ss`, and it is the machine's own clock rather than UTC —
/// a player asking the time wants the time where the server is.
fn handle_server_time(session: &Session) -> Action {
    let now = chrono::Local::now().format("%d/%m/%Y %H:%M:%S").to_string();
    Action::Reply(vec![encode_client_message(session.client_id, &now)])
}

/// `0x3E04`: make a character in one of the three slots.
///
/// The reply is the whole character list either way, because that is what the
/// client redraws the selection screen from; a refusal also carries the
/// sentence that says why.
async fn handle_create_character(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    let Some(request) = creation::CreateCharacter::parse(&message.body) else {
        warn!(size = message.body.len(), "0x3E04 packet too short");
        return Action::Ignore;
    };

    let Some(account) = session.account.as_ref() else {
        warn!("0x3E04 before logging in");
        return Action::Ignore;
    };
    let username = account.username.clone();

    let class_number = (request.class_index / 10).clamp(1, 6);
    let outcome = creation::create(
        &request,
        &account.characters,
        |name| state.store.name_taken(name),
        state.template(class_number),
    );

    let mut character = match outcome {
        Ok(character) => character,
        Err(e) => {
            info!(user = %username, name = %request.name, error = %e, "character refused");
            return Action::Reply(vec![
                encode_client_message(session.client_id, &e.message()),
                encode_char_list(account, session.client_id, PACKET_TIME),
            ]);
        }
    };

    // The database hands back the id, and without it nothing this character
    // does later can be saved. A failure here has to reach the player rather
    // than leave a character that exists until the next restart.
    if let Some(db) = state.db() {
        match db.insert_character(account.id as i64, &character).await {
            Ok(id) => character.id = id,
            Err(e) => {
                // The whole chain, not only the outermost context: the
                // one time this fired, the reason was two layers down and
                // the log said nothing useful.
                warn!(
                    user = %username,
                    name = %character.name,
                    error = format!("{e:#}"),
                    "character not stored"
                );
                return Action::Reply(vec![
                    encode_client_message(session.client_id, "The character could not be saved."),
                    encode_char_list(account, session.client_id, PACKET_TIME),
                ]);
            }
        }
    }

    info!(
        user = %username,
        name = %character.name,
        slot = character.slot,
        class = character.class_index,
        "character created"
    );

    state.store.add_character(&username, character);

    // Read back rather than patched in place, so the list the client gets is
    // the one the store holds.
    let account = state.store.get(&username).unwrap_or_else(|| account.clone());
    session.account = Some(account.clone());
    Action::Reply(vec![encode_char_list(&account, session.client_id, PACKET_TIME)])
}


/// `0x668`: back to the character selection screen.
///
/// The original closes the socket and lets the client reconnect, with a
/// comment from whoever wrote it saying it is a hack and that they never
/// found the real logout packet. It is still the right shape: the connection
/// carries a character from the moment one is chosen, so leaving the world
/// means starting a new connection. Disconnecting here runs the same teardown
/// as any other, which is what saves the session.
fn handle_back_to_character_select(session: &Session) -> Action {
    match session.character.as_ref() {
        Some(character) => info!(character = %character.name, "leaving for the character list"),
        None => debug!("0x668 before a character was chosen"),
    }
    Action::Disconnect
}

/// `0x3E01`: delete the character in a slot.
///
/// The row is marked deleted rather than removed. A player who deletes the
/// wrong character has lost it either way as far as the game is concerned,
/// but a mistake stays recoverable by hand, and the name stays taken, which
/// is what stops somebody else claiming it a minute later.
async fn handle_delete_character(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    let Some(request) = creation::DeleteCharacter::parse(&message.body) else {
        warn!(size = message.body.len(), "0x3E01 packet too short");
        return Action::Ignore;
    };

    let Some(account) = session.account.as_ref() else {
        warn!("0x3E01 before logging in");
        return Action::Ignore;
    };
    let username = account.username.clone();
    let slot = request.slot as usize;

    let Some(doomed) = account.characters.iter().find(|c| c.slot == slot).cloned() else {
        debug!(user = %username, slot, "0x3E01 for an empty slot");
        return Action::Reply(vec![encode_char_list(
            account,
            session.client_id,
            PACKET_TIME,
        )]);
    };

    // A character being played cannot be deleted from under itself.
    if session.character.as_ref().is_some_and(|c| c.id == doomed.id) {
        return Action::Reply(vec![
            encode_client_message(session.client_id, "You cannot delete the character you are playing."),
            encode_char_list(account, session.client_id, PACKET_TIME),
        ]);
    }

    if let Some(db) = state.db() {
        if let Err(e) = db.soft_delete_character(doomed.id).await {
            warn!(
                user = %username,
                name = %doomed.name,
                error = format!("{e:#}"),
                "character not deleted"
            );
            return Action::Reply(vec![
                encode_client_message(session.client_id, "The character could not be deleted."),
                encode_char_list(account, session.client_id, PACKET_TIME),
            ]);
        }
    }

    info!(user = %username, name = %doomed.name, slot, "character deleted");
    state.store.remove_character(&username, slot);

    let account = state.store.get(&username).unwrap_or_else(|| account.clone());
    session.account = Some(account.clone());
    Action::Reply(vec![encode_char_list(&account, session.client_id, PACKET_TIME)])
}



/// The skills a character has, worked out from the table.
fn known_skills(state: &State, character: &Character) -> Vec<usize> {
    ability::bar_of(&state.skills, character.class_number() as u32, character.level as u32)
}

/// `TSendSkillsPacket` (`0x106`): the forty skills the client draws on the
/// bar. Same opcode and size as the shop window, which is why it must only
/// go out when the client is expecting a skill list.
fn encode_skill_list(client_id: u16, skills: &[usize]) -> Vec<u8> {
    encode_skill_list_from(client_id, 0, skills)
}

/// The same list, said to come from an NPC. The send type is what makes the
/// client open a trainer instead of redrawing the bar.
fn encode_skill_list_from(client_id: u16, npc: u16, skills: &[usize]) -> Vec<u8> {
    let mut body = Vec::with_capacity(SKILLS_SIZE - MIN_FRAME);
    body.extend_from_slice(&npc.to_le_bytes());
    let send_type: u16 = if npc > 0 { SKILL_LIST_FROM_NPC } else { 0 };
    body.extend_from_slice(&send_type.to_le_bytes());
    for slot in 0..ability::SKILL_SLOTS {
        let id = skills.get(slot).copied().unwrap_or(0) as u16;
        body.extend_from_slice(&id.to_le_bytes());
    }

    debug_assert_eq!(body.len() + MIN_FRAME, SKILLS_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: ability::OP_SKILL_LIST, time: 0, body },
        rand::random(),
    )
}

/// The `0x107` body: sixty learned-skill words, unspent points, and `0xCCCC`.
const SKILLS_LEVEL_SIZE: usize = MIN_FRAME + 60 * 2 + 2 + 2;
/// The trailer the original writes after the skill points (`Unk := $CCCC`).
const SKILLS_LEVEL_TRAILER: u16 = 0xCCCC;

/// The record's skill list is 120 bytes, sixty words.
const SKILL_LIST_BYTES: usize = 120;
/// The last rank a slot has, and the one whose word does not fit in two bytes.
const LAST_RANK: u16 = 16;

/// `TPlayer.SetPlayerSkills` (`Mob/Player.pas:7079`): the sixty words of the
/// record's skill list, **built and never stored**.
///
/// # The word is not a level
///
/// It is every rank up to the one held, as bits: `GetSkillLevel` computes
/// `2 ^ (rank + 1) - 2`, so rank one is `10`, rank two `110`, rank three
/// `1110`. And the rank it raises two to is the *table's* own column for the
/// id in use, not the count this server keeps.
///
/// Sending the count instead is a loop with no way out, and it is worth
/// spelling out because it looks like nothing on the wire. A fresh character
/// holds level one of its first advanced skill, whose id the table calls rank
/// two, so the word is 6 and the client asks to buy rank three. Send it 1 and
/// the client reads rank one and asks to buy rank two — which is the same id
/// it just bought. It asks again after every purchase, for ever, and the rank
/// never moves.
///
/// A basic is `2` whatever it has learned, which is the original writing the
/// same formula's answer for rank one by hand.
fn set_player_skills(character: &Character, skills: &SkillTable) -> [u8; SKILL_LIST_BYTES] {
    let mut list = [0u8; SKILL_LIST_BYTES];

    for i in 0..BASIC_SKILL_COUNT {
        if character.skill_list[i] != 0 {
            list[i * 2..i * 2 + 2].copy_from_slice(&BASIC_SKILL_LEARNED.to_le_bytes());
        }
    }

    let class = character.class_number() as u32;
    for i in 0..ability::SKILL_SLOTS {
        let slot = BASIC_SKILL_COUNT + i;
        let level = character.skill_list[slot];
        if level == 0 {
            continue;
        }

        // `Others[I].Index + (Others[I].Level - 1)`: the record keeps the
        // first rank's id and a level, and the rank in use is that many ids
        // along.
        let id = ability::skill_index(class, slot + 1, 1) + level as usize - 1;
        let (size, mut value) = learned_ranks(skills, id);

        // The original's own patch for the slot before this one having spilled
        // four bytes where two were expected. Copied because the client is
        // reading what the original wrote, spill and all.
        if i > 0 && character.skill_list[slot - 1] == LAST_RANK {
            value += 1;
        }

        let at = slot * 2;
        debug_assert!(at + size <= SKILL_LIST_BYTES, "the skill list would run off its end");
        list[at..at + size].copy_from_slice(&value.to_le_bytes()[..size]);
    }
    list
}

/// `TSkillFunctions.GetSkillLevel` (`Functions/SkillFunctions.pas:38`): every
/// rank up to this id's, as bits, and how many bytes of it to write.
///
/// Two bytes until rank fifteen and four from sixteen, which is the original's
/// own `case` and the reason a maxed skill writes over the word after it.
fn learned_ranks(skills: &SkillTable, id: usize) -> (usize, u32) {
    let rank = skills.get(id).map_or(0, |skill| skill.rank());
    let value = 2u32.checked_pow(rank + 1).map_or(0, |power| power - 2);
    let size = match value {
        0..=65535 => 2,
        65536..=131080 => 4,
        // `Result` starts at nought and no case matches, so nothing is
        // written at all.
        _ => 0,
    };
    (size, value)
}

/// `0x107` (`SendPlayerSkillsLevel`): the learned state of every skill.
///
/// The sixty words are what [`set_player_skills`] builds, which is what tells
/// the client a skill may be cast, which rank of it, and greys the others out.
/// Without it the client draws the whole tree as if available and lets the
/// player press skills the server then refuses. The word after is how many
/// skill points are unspent, which the original takes from the score.
fn encode_skills_level(character: &Character, client_id: u16, skills: &SkillTable) -> Vec<u8> {
    let mut body = Vec::with_capacity(SKILLS_LEVEL_SIZE - MIN_FRAME);
    body.extend_from_slice(&set_player_skills(character, skills));
    body.extend_from_slice(&character.skill_points.to_le_bytes());
    body.extend_from_slice(&SKILLS_LEVEL_TRAILER.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, SKILLS_LEVEL_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_SKILLS_LEVEL, time: 0, body },
        rand::random(),
    )
}

/// `0x320`: the player used a skill.
///
/// The original spends the mana, checks the cooldown and the target, relays
/// the packet to everyone who can see it so they get the animation, and only
/// then works out what it did (`PacketHandlers.pas:7550`). Same order here:
/// the relay is what makes a spell look like it happened.
fn handle_use_skill(state: &State, session: &mut Session, message: &Message) -> Action {
    cast_skill(state, session, message, ability::Named::ByTheClient)
}

/// The casting itself, told who chose the skill.
///
/// A mount's skills come through here with the server as the one that named
/// them, because they belong to no class and would fail an ownership test
/// that exists to stop a client asking for a skill it never learned.
fn cast_skill(
    state: &State,
    session: &mut Session,
    message: &Message,
    named: ability::Named,
) -> Action {
    let Some(request) = ability::UseSkill::parse(&message.body) else {
        warn!(size = message.body.len(), "0x320 packet too short");
        return Action::Ignore;
    };

    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let client_id = session.client_id;
    let at = (character.x as f32, character.y as f32);

    // A skill aimed at something needs that something to exist. Its position
    // comes from the world, never from the packet.
    let target = state.world.mob(request.target as u16).filter(|m| m.is_alive());
    let target_at = target.as_ref().map(|m| m.position());

    let caster = ability::Caster {
        class_number: character.class_number() as u32,
        level: character.level as u32,
        mana: session.cur_mp,
        at,
    };

    let now = std::time::Instant::now();
    let checked = match named {
        ability::Named::ByTheClient => ability::check,
        ability::Named::ByTheServer => ability::check_chosen,
    };
    let cast = match checked(
        &state.skills,
        &caster,
        &session.cooldowns,
        request.skill,
        target_at,
        now,
    ) {
        Ok(cast) => cast,
        Err(e) => {
            debug!(skill = request.skill, error = %e, "cast refused");
            return Action::Reply(vec![encode_client_message(client_id, &e.message())]);
        }
    };

    session.cur_mp = session.cur_mp.saturating_sub(cast.mana);
    session.cooldowns.start(cast.family, cast.cooldown, now);
    debug!(
        skill = cast.skill,
        cast_ms = state.skills.get(cast.skill).map(|s| s.cast_time_ms()).unwrap_or(0),
        target = request.target,
        "cast started"
    );

    // Casting stands the player up, the same way walking does (`UseSkill`
    // clears `CurrentAction` too).
    session.action = 0;
    state.world.act(client_id, 0);

    // Everyone who can see the caster sees the cast, animation and all.
    let relay = frame::encode(
        &Message {
            sender: client_id,
            opcode: ability::OP_USE_SKILL,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(client_id, relay.clone());

    let (max_hp, max_mp) = vitals(session.character.as_ref().expect("checked above"));
    let mut frames = vec![relay, encode_hp_mp(
        session.character.as_ref().expect("checked above"),
        client_id,
        session.cur_hp.min(max_hp),
        session.cur_mp.min(max_mp),
    )];

    // A skill that hurts something works out its damage the same way a swing
    // does, with the skill's own number as the floor rather than the level.
    let Some(target) = target else {
        return Action::Reply(frames);
    };
    if !cast.is_aggressive {
        return Action::Reply(frames);
    }

    // A spell leans on the caster's magic attack rather than its swing.
    let stats = stats::of(character, &state.items, &session.effects(state));
    let blow = combat::swing_with(
        character.level,
        target.level,
        cast.damage + stats::base_damage(stats.magic_attack, target.level as u32),
        &mut rand::thread_rng(),
    );
    let Some((target, killed)) = state.world.wound_mob(target.id, blow.damage, client_id, now) else {
        return Action::Reply(frames);
    };

    let (_, flinch) = animations_of(state, cast.skill as u16);
    let report = combat::Damage {
        skill: cast.skill as u16,
        attacker: client_id,
        attacker_at: at,
        attacker_hp: session.cur_hp.min(max_hp),
        animation: cast.animation as u16,
        target_animation: flinch,
        target: target.id,
        target_hp: target.hp,
        blow,
        at: target.position(),
    };
    let damage_frame = encode_damage(&report);
    state.world.send_to_visible(client_id, damage_frame.clone());
    frames.push(damage_frame);

    if killed {
        info!(monster = %target.name, skill = cast.skill, "killed with a skill");
        frames.extend(reward_for(state, session, &target));
        session.visible_mobs.remove(&target.id);
    }
    Action::Reply(frames)
}


/// Sends the buff list again while something endless is running.
///
/// A buff with no end is drawn from a window that the client counts down, so
/// left alone the icon would leave the bar while the buff was still on. This
/// tops the window back up: one small packet a minute, and only while there
/// is an endless buff to keep alive.
fn top_up_endless(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    if !session.buffs.any_endless(&state.skills) {
        return Vec::new();
    }
    let due = session
        .buffs_sent_at
        .is_none_or(|at| at.elapsed() >= crate::buffs::ENDLESS_REFRESH);
    if !due {
        return Vec::new();
    }
    session.buffs_sent_at = Some(std::time::Instant::now());
    vec![encode_buffs(session.client_id, &session.buffs, &state.skills)]
}

/// Takes off whatever has run out, and redraws what depended on it.
///
/// `RefreshBuffs` sends fresh health, status and points when anything went,
/// and nothing at all when nothing did. Nothing runs a clock: this rides the
/// heartbeat the client sends twice a second, which is close enough for a
/// buff measured in minutes.
fn drop_spent_buffs(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let now = std::time::SystemTime::now();
    if session.buffs.expire(&state.skills, now) == 0 {
        return top_up_endless(state, session);
    }
    session.buffs_sent_at = Some(std::time::Instant::now());
    let client_id = session.client_id;
    let mut frames = vec![encode_buffs(client_id, &session.buffs, &state.skills)];
    session.buffs_sent_at = Some(std::time::Instant::now());
    let effects = session.effects(state);
    if let Some(character) = session.character.as_ref() {
        let (max_hp, max_mp) = vitals(character);
        frames.push(encode_hp_mp(
            character,
            client_id,
            session.cur_hp.min(max_hp),
            session.cur_mp.min(max_mp),
        ));
        frames.push(encode_refresh_status(character, &state.items, &effects));
        frames.push(encode_refresh_point(character));
    }
    info!("a buff ran out");
    frames
}

/// Applies whatever monsters landed on this player since its last packet.
///
/// The world tick cannot reach into a session, so it leaves the damage in the
/// world and this takes it. Being late by up to half a second is fine: the
/// client draws its own health bar from what it is told, and it is told here.
fn collect_blows(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let blows = state.world.take_incoming(session.client_id);
    if blows.is_empty() || session.character.is_none() {
        return Vec::new();
    }
    if session.dead {
        return Vec::new();
    }

    let client_id = session.client_id;
    let stats = stats::of(
        session.character.as_ref().expect("checked above"),
        &state.items,
        &session.effects(state),
    );
    let mut frames = Vec::new();

    for blow in blows {
        let damage = stats::base_damage(blow.damage, stats.defence);
        session.cur_hp = session.cur_hp.saturating_sub(damage);

        let (swing, flinch) = animations_of(state, blow.skill);
        let attacker_at = state
            .world
            .mob(blow.attacker)
            .map(|m| m.position())
            .unwrap_or_default();

        let report = combat::Damage {
            skill: blow.skill,
            attacker: blow.attacker,
            attacker_at,
            attacker_hp: state.world.mob(blow.attacker).map(|m| m.hp).unwrap_or_default(),
            animation: swing,
            target_animation: flinch,
            target: client_id,
            target_hp: session.cur_hp,
            blow: combat::Blow { damage, kind: combat::DAMAGE_NORMAL },
            at: session
                .character
                .as_ref()
                .map(|c| (c.x as f32, c.y as f32))
                .unwrap_or_default(),
        };
        frames.push(encode_damage(&report));

        if session.cur_hp == 0 {
            frames.extend(die(state, session, &stats));
            break;
        }
    }
    frames
}

/// What happens when a player runs out of health.
///
/// The client is told the health is gone and the character is marked down.
/// Nothing is taken away: the original has a death penalty, and inventing one
/// is a decision for whoever runs the server rather than for this.
fn die(state: &State, session: &mut Session, stats: &stats::Stats) -> Vec<Vec<u8>> {
    session.dead = true;
    session.cur_hp = 0;

    let name = session.character.as_ref().map(|c| c.name.clone()).unwrap_or_default();
    info!(character = %name, "died");

    let mut frames = vec![encode_hp_mp(
        session.character.as_ref().expect("checked by the caller"),
        session.client_id,
        0,
        session.cur_mp.min(stats.max_mp),
    )];

    // Everyone nearby sees them go down.
    let frame = encode_remove_mob(session.client_id, DELETE_NORMAL);
    state.world.send_to_visible(session.client_id, frame.clone());
    frames.push(frame);
    frames
}

/// `0x303`: get up again.
///
/// The original revives at the nearest save point; without those read yet,
/// this is the town the character was created in, which is where a save point
/// would be anyway.
fn handle_revive(state: &State, session: &mut Session, _message: &Message) -> Action {
    if !session.dead {
        debug!("0x303 from somebody who is not dead");
        return Action::Ignore;
    }

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let (x, y) = creation::TOWN_FIRST;
    character.x = x;
    character.y = y;

    let character = session.character.as_ref().expect("checked above").clone();
    let stats = stats::of(&character, &state.items, &session.effects(state));

    session.dead = false;
    session.cur_hp = stats.max_hp;
    session.cur_mp = stats.max_mp;
    session.dirty = true;
    session.visible.clear();
    session.visible_mobs.clear();
    session.visible_npcs.clear();

    state.world.enter(session.client_id, character.clone());
    state.world.move_to(session.client_id, x, y);
    info!(character = %character.name, "got up");

    let mut frames = vec![
        encode_spawn_as(&character, session.client_id, SPAWN_TELEPORT_IN, stats.speed_move),
        encode_hp_mp(&character, session.client_id, session.cur_hp, session.cur_mp),
    ];
    frames.extend(refresh_npc_visibility(state, session));
    frames.extend(refresh_mob_visibility(state, session));
    Action::Reply(frames)
}

/// `0x302`: the player swung at something.
///
/// Only monsters can be hit. Attacking another player needs a duel or a war
/// to be agreed first, and neither exists yet; hitting an NPC is refused by
/// the original too.
/// A skill the caster aimed at themselves, arriving as the cast finishes.
///
/// Only the lasting ones do anything here: a skill with a duration becomes a
/// buff, which is what the mount is. One with none is a swing at yourself and
/// there is nothing to do with it.
///
/// The mana and the cooldown were taken when the bar started, so they are not
/// taken again — this is the second half of one cast, not a second cast.
/// The only target type the original treats as a cast on oneself.
///
/// `Mob/BaseMob.pas:5754` branches on exactly this and nothing else: type one
/// goes to `SelfBuffSkill`, everything else is aimed at something. Two
/// thousand of the table's skills are type one and eight hundred are type
/// four, so the two branches are both well travelled and telling them apart by
/// the target id in the packet gets the second one wrong.
const TARGET_TYPE_SELF: u32 = 1;

fn finish_self_cast(state: &State, session: &mut Session, skill: usize) -> Action {
    let Some(def) = state.skills.get(skill) else {
        debug!(skill, "cast finished, but the table has no such skill");
        return Action::Ignore;
    };
    if def.duration_secs() == 0 {
        // Nothing to hold on to, but the client is waiting to be let go of:
        // the animation still goes out below.
        debug!(skill, "cast finished on self with nothing lasting to show for it");
        return play_self_cast(state, session, skill, def.self_animation() as u16);
    }
    debug!(skill, "cast finished");
    let animation = def.self_animation() as u16;

    let mut frames = grant_buff(state, session, skill);
    if let Action::Reply(played) = play_self_cast(state, session, skill, animation) {
        frames.splice(0..0, played);
    }
    Action::Reply(frames)
}

/// The animation everyone watching plays when a cast lands on its caster.
///
/// The original builds a fresh `0x302` rather than echoing the one it got, and
/// fills the animation from the skill's own `SelfAnimation`: the client sends
/// nothing useful in that field, so a cast finished without this leaves the
/// caster standing still while the spell goes off.
///
/// It goes out even for a skill that leaves nothing behind. The client is
/// waiting to be told the cast is over, and a cast it is never let go of is a
/// client that stops moving.
fn play_self_cast(
    state: &State,
    session: &mut Session,
    skill: usize,
    animation: u16,
) -> Action {
    // Where the caster stands comes from the world, not from the packet.
    let at = session
        .character
        .as_ref()
        .map(|c| (c.x as f32, c.y as f32))
        .unwrap_or((0.0, 0.0));

    let played = combat::Attack {
        target: session.client_id,
        animation,
        skill: skill as u16,
        from: at,
        at,
    };
    let relay = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: combat::OP_ATTACK,
            time: 0,
            body: played.to_body(),
        },
        rand::random(),
    );
    state.world.send_to_visible(session.client_id, relay.clone());
    Action::Reply(vec![relay])
}

fn handle_attack(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = combat::Attack::parse(&message.body) else {
        warn!(size = message.body.len(), "0x302 packet too short");
        return Action::Ignore;
    };

    if session.dead {
        return Action::Ignore;
    }
    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let at = (character.x as f32, character.y as f32);
    let level = character.level;

    // A skill with a cast time finishes here rather than where it started.
    //
    // The client sends `0x320` when the bar begins and this when it fills, so
    // for anything with a `CastTime` the effect belongs to this packet and not
    // to the other one — `AttackTarget` is where the original applies it too,
    // told by `ByUseSkill` which of the two it came from.
    //
    // Which of the two branches it takes is decided by the skill, never by the
    // packet: `if (DataSkill^.TargetType = 1) then SelfBuffSkill else [Target]`
    // (`Mob/BaseMob.pas:5754`). Type one is the only self-cast there is, and it
    // is most of the table — 2157 skills, every blessing, mount and stance.
    //
    // Reading the target id instead was close enough to work and wrong in the
    // case that mattered. A skill aimed at nothing arrives carrying the
    // caster's own id, so an attack the player had not aimed at anybody was
    // handed to the player as a buff. Skill 289 is a sixty-second attack with
    // a debuff on it; cast that way it rooted the caster where they stood, and
    // the client was obeying us exactly.
    let cast_def = state.skills.get(request.skill as usize);
    if request.skill != 0 && cast_def.is_some_and(|s| s.target_type() == TARGET_TYPE_SELF) {
        return finish_self_cast(state, session, request.skill as usize);
    }

    // The position comes from the world, never from the packet: the client
    // sends where it thinks it is, and a modified one would reach across the
    // map by lying about it.
    // A cast that lands on nothing still has to be let go of. The client is
    // waiting to be told the bar it filled is finished with, and one it is
    // never told about is one that stops sending anything but which way it is
    // facing. Answering with nothing at all is what froze a session solid.
    let animation = cast_def.map(|s| s.self_animation() as u16).unwrap_or(0);
    let let_go = |session: &mut Session, why: &str| {
        debug!(target = request.target, skill = request.skill, why, "cast landed on nothing");
        play_self_cast(state, session, request.skill as usize, animation)
    };

    let Some(target) = state.world.mob(request.target) else {
        return let_go(session, "not a monster");
    };
    if !target.is_alive() {
        return let_go(session, "already dead");
    }
    if !within(at, target.position(), combat::MELEE_RANGE) {
        return let_go(session, "out of reach");
    }

    // Attack comes off the character and its gear now, and the monster's
    // level stands in for the armour it is not wearing.
    let stats = stats::of(
        session.character.as_ref().expect("checked above"),
        &state.items,
        &session.effects(state),
    );
    let blow = combat::swing_with(
        level,
        target.level,
        stats::base_damage(stats.attack, target.level as u32),
        &mut rand::thread_rng(),
    );
    let Some((target, killed)) =
        state.world.wound_mob(
            request.target,
            blow.damage,
            session.client_id,
            std::time::Instant::now(),
        )
    else {
        // Somebody else landed the last blow between the checks above and
        // here. Theirs, not ours.
        return Action::Ignore;
    };

    let (max_hp, _) = vitals(session.character.as_ref().expect("checked above"));
    let (_, flinch) = animations_of(state, request.skill);
    let report = combat::Damage {
        skill: request.skill,
        attacker: session.client_id,
        attacker_at: at,
        attacker_hp: session.cur_hp.min(max_hp),
        animation: request.animation,
        target_animation: flinch,
        target: target.id,
        target_hp: target.hp,
        blow,
        at: target.position(),
    };
    let frame = encode_damage(&report);

    // Everyone who can see the fight sees it. A fight only the person
    // swinging can see is a fight that looks like it never happened.
    state.world.send_to_visible(session.client_id, frame.clone());

    let mut frames = vec![frame];
    if killed {
        info!(
            monster = %target.name,
            level = target.level,
            experience = target.experience,
            "killed"
        );
        frames.extend(reward_for(state, session, &target));
        session.visible_mobs.remove(&target.id);
    }
    Action::Reply(frames)
}

/// What a kill is worth: experience, whatever levels that buys, and whatever
/// the monster was carrying.
fn reward_for(state: &State, session: &mut Session, target: &crate::mob::Mob) -> Vec<Vec<u8>> {
    // The companion is paid here rather than at either of the places a kill is
    // noticed. There are two of them -- a swing and a spell -- and paying the
    // pran at one of them is exactly the mistake that was made: every monster
    // killed with a weapon fed nobody.
    let mut frames = reward_the_pran(state, session, target.experience as u64);

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return frames;
    };

    character.exp = character.exp.saturating_add(target.experience as u64);
    session.dirty = true;

    // The curve decides the level, not a running count of kills, so a
    // character whose experience is edited in the database lands where that
    // experience says it should.
    let gained = state.levels.levels_gained(character.level, character.exp);
    // A character stops at its own tier's wall, not at the end of the curve.
    // Experience keeps piling up while it waits there, so being promoted late
    // does not cost anything that was earned in the meantime.
    let cap = promotion::level_cap(character.tier);
    // Anything the companion earned goes out with the rest.

    if gained > 0 && character.level < cap {
        let was = character.level;
        character.level = character.level.saturating_add(gained).min(cap);

        // A level is worth points, and the original hands them out one level
        // at a time: `AddExp` loops on `AddLevel` rather than adding several
        // at once, so two levels from one kill pay twice. Counting them here
        // rather than off the new level keeps that true.
        let (mut skill, mut status) = (0u16, 0u16);
        for level in was + 1..=character.level {
            let (s, t) = crate::store::points_for_reaching(level);
            skill += s;
            status += t;
        }
        character.skill_points = character.skill_points.saturating_add(skill);
        character.attributes[FREE_POINTS] =
            character.attributes[FREE_POINTS].saturating_add(status);
        info!(
            character = %character.name,
            level = character.level,
            cap,
            skill,
            status,
            "levelled up"
        );

        // Health and mana come back full: a level is the one moment the game
        // hands them over, and arriving at a new level nearly dead is a
        // punishment for winning.
        let stats = stats::of(character, &state.items, &Effects::none());
        session.cur_hp = stats.max_hp;
        session.cur_mp = stats.max_mp;
        frames.push(encode_hp_mp(
            session.character.as_ref().expect("checked above"),
            client_id,
            session.cur_hp,
            session.cur_mp,
        ));

        // And what the level was worth. `AddLevel` sends this straight after
        // the vitals; without it the points are in the record and the two
        // windows that spend them keep drawing the old count until a relog.
        frames.push(encode_refresh_point(session.character.as_ref().expect("checked above")));

        // The burst everyone on screen sees. The original plays it on the
        // player for everyone who can see them, so it goes out to the world
        // and comes back to the killer in the same reply.
        let effect = encode_effect(client_id, EFFECT_LEVEL_UP);
        state.world.send_to_visible(client_id, effect.clone());
        frames.push(effect);
    }

    frames.push(encode_level(session.character.as_ref().expect("checked above"), client_id));
    frames.extend(loot_from(state, session, target));
    frames
}

/// What the monster was carrying, if anything, straight into the bag.
///
/// The original drops it on the ground for the player to pick up, which needs
/// map items and the packets that go with them. Handing it over is the same
/// outcome with one fewer system, and it is honest about being a shortcut:
/// nobody else can take it, and a full bag loses it.
fn loot_from(state: &State, session: &mut Session, target: &crate::mob::Mob) -> Vec<Vec<u8>> {
    let band = state.drops.band_for(target.drop_index);
    let mut rng = rand::thread_rng();

    let Some(id) = aika_data::drops::roll(
        band,
        rand::Rng::gen_range(&mut rng, 1..=100),
        rand::Rng::gen_range(&mut rng, 1..=100),
        rand::Rng::gen_range(&mut rng, 0..usize::MAX),
    ) else {
        return Vec::new();
    };

    if state.items.get(id as usize).is_none() {
        debug!(item = id, "a drop table names an item the item table does not");
        return Vec::new();
    }

    let mut dropped = Item { refine: 1, ..Item::from_table(id, &state.items) };
    // Same clock as a purchase: what a monster leaves behind can be a timed
    // item too, and it starts running when it is picked up.
    expiry::stamp(&mut dropped, &state.items, expiry::now());

    let Some(character) = session.character.as_mut() else {
        return Vec::new();
    };
    match character.items.add(dropped) {
        Ok(slot) => {
            let item = character.items.get(inventory::BAG, slot).cloned().unwrap_or_default();
            info!(item = id, slot, monster = %target.name, "looted");
            vec![encode_refresh_item(inventory::BAG, slot, &item, true)]
        }
        Err(_) => vec![encode_client_message(
            session.client_id,
            "Your bag is full, so the drop was lost.",
        )],
    }
}

/// The two animations a blow plays: the swing and the flinch, both from the
/// skill it was made with (`Mob/BaseMob.pas:9842`).
///
/// A blow with no skill behind it plays nothing, which is what the original
/// does for a monster whose kind lists none.
fn animations_of(state: &State, skill: u16) -> (u16, u8) {
    let Some(def) = state.skills.get(skill as usize) else {
        return (0, 0);
    };
    (def.animation() as u16, def.target_animation() as u8)
}

/// `TRecvDamagePacket` (`0x102`): who hit what, for how much, and what is
/// left of it.
fn encode_damage(damage: &combat::Damage) -> Vec<u8> {
    let body = damage.to_body();
    debug_assert_eq!(body.len() + MIN_FRAME, combat::DAMAGE_SIZE);
    frame::encode(
        &Message { sender: damage.attacker, opcode: combat::OP_DAMAGE, time: 0, body },
        rand::random(),
    )
}

/// A monster's `0x349`. Closer to a player's than to an NPC's: the original
/// writes the same `Unk0` as for a player and does not flag it as a service
/// (`Mob/BaseMob.pas:3045`). What tells the client it is a monster is the id
/// range it arrives in.
fn encode_mob_spawn(mob: &crate::mob::Mob) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    // The name is the digits of a string index, the same convention the NPCs
    // use: the client owns the words.
    write_fixed_str(&mut body[off::NAME..off::NAME + 16], &mob.name_index.to_string());

    for (i, model) in mob.model.iter().enumerate() {
        put16(&mut body, off::EQUIP + i * 2, *model);
    }

    body[off::POSITION_X..off::POSITION_X + 4].copy_from_slice(&mob.x.to_le_bytes());
    body[off::POSITION_Y..off::POSITION_Y + 4].copy_from_slice(&mob.y.to_le_bytes());
    put32(&mut body, off::ROTATION, mob.rotation as u32);

    put32(&mut body, off::MAX_HP, mob.max_hp);
    put32(&mut body, off::MAX_MP, mob.max_hp);
    put32(&mut body, off::CUR_HP, mob.hp);
    put32(&mut body, off::CUR_MP, mob.max_hp);

    body[off::UNK0] = SPAWN_UNK0;
    body[off::SPEED_MOVE] = MOB_SPEED_MOVE;
    body[off::SPAWN_TYPE] = mob.spawn_type;
    body[off::SIZES..off::SIZES + 3].copy_from_slice(&mob.sizes);

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: mob.id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `TSendClientIndexPacket` (`0x117`): the id of the mob the player is.
fn encode_client_index(client_id: u16) -> Vec<u8> {
    encode_effect(client_id, 0)
}

/// The same packet as an effect over somebody's head (`TPlayer.SendEffect`).
///
/// `Index` is who it plays on and `Effect` which one; effect 1 is the burst a
/// character gives off when it gains a level. The original sends it to
/// everyone who can see the player, themselves included, so a level-up is
/// visible to the whole screen and not just the one who earned it.
fn encode_effect(client_id: u16, effect: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(CLIENT_INDEX_SIZE - MIN_FRAME);
    body.extend_from_slice(&(client_id as u32).to_le_bytes());
    body.extend_from_slice(&effect.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, CLIENT_INDEX_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CLIENT_INDEX, time: 0, body },
        rand::random(),
    )
}

/// `0x3E02`: the name a player typed for their companion.
///
/// A pran is named once and keeps it. `RenamePran` does not read the slot the
/// client sends -- it names the first pran that has no name, and refuses when
/// they all do, so there is no way to change one afterwards.
///
/// The answer is the question sent back with the account filled in, followed
/// by the two chest slots the pran centre draws from. Those two are refreshed
/// here for the same reason `SendStorage` refreshes them: the packet that
/// carries the chest does not reach them.
async fn handle_rename_pran(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    let Some(asked) = pran::Rename::parse(&message.body) else {
        warn!(size = message.body.len(), "0x3E02 packet too short");
        return Action::Ignore;
    };
    let client_id = session.client_id;
    let refuse = |why: &str| Action::Reply(vec![encode_client_message(client_id, why)]);

    if !pran::name_is_allowed(&asked.name) {
        return refuse("That name cannot be used.");
    }
    if state.pran_name_taken(&asked.name).await {
        return refuse("That name is already taken.");
    }

    let Some(account) = session.account.as_mut() else {
        return Action::Ignore;
    };
    let Some(unnamed) = account.prans.iter_mut().find(|p| p.name.is_empty()) else {
        // The client only asks when it believes the pran has no name, and
        // nothing ever tells it otherwise: the name travels in `0x907`, which
        // goes out when a pran is summoned and at no other time. A player who
        // logs back in with a named pran still in the chest asks again, is
        // refused, and asks again -- and an unnamed pran is one the client
        // will not let out of the chest, so the loop has no exit.
        //
        // The refusal is the original's. Saying what it is called alongside is
        // ours, and it is the smallest thing that ends the loop: it can only
        // fire where the client has already proved it is out of date.
        let mut frames = vec![encode_client_message(
            client_id,
            "All of your prans already have a name.",
        )];
        if let Some(pran) = account.prans.first() {
            frames.push(frame::encode(
                &Message {
                    sender: dialog::FIXED_INDEX,
                    opcode: pran::OP_WORLD,
                    time: 0,
                    body: pran::world_body(pran),
                },
                rand::random(),
            ));
        }
        return Action::Reply(frames);
    };

    unnamed.name = asked.name.clone();
    session.dirty = true;
    let account_id = account.id;
    info!(name = %asked.name, "a pran was named");

    let mut frames = vec![frame::encode(
        &Message {
            sender: client_id,
            opcode: pran::OP_RENAME,
            time: message.time,
            body: asked.to_body(account_id),
        },
        rand::random(),
    )];
    for slot in inventory::STORAGE_PRAN_SLOTS {
        let item = account.storage.get(inventory::STORAGE, slot).cloned().unwrap_or_default();
        frames.push(encode_refresh_item(inventory::STORAGE, slot, &item, false));
    }
    Action::Reply(frames)
}
/// A companion's share of what its owner just killed.
///
/// A fifth of it, and only while the pran is out: the original hands it over
/// in the switch on `SpawnedPran`, so a pran left in the chest earns nothing
/// (`Mob/BaseMob.pas:6177`).
///
/// The level goes back on its own packet. That is the whole reason a pran
/// never changed shape here: `OP_WORLD` describes everything about one
/// except its level, and the client does not work the level out from the
/// experience -- it waits to be told, and reads level 1 until it is.
fn reward_the_pran(state: &State, session: &mut Session, experience: u64) -> Vec<Vec<u8>> {
    if session.pran_out.is_none() {
        return Vec::new();
    }
    let share = pran::share_of_kill(experience);
    if share == 0 {
        return Vec::new();
    }

    let client_id = session.client_id;
    let Some(account) = session.account.as_mut() else {
        return Vec::new();
    };
    let Some(at) = session.pran_out else {
        return Vec::new();
    };
    let Some(pran) = account.prans.get_mut(at) else {
        return Vec::new();
    };

    let grew = pran::add_exp(pran, share, &state.pran_levels);
    session.dirty = true;

    let mut frames = vec![frame::encode(
        &Message {
            sender: dialog::FIXED_INDEX,
            opcode: pran::OP_LEVEL,
            time: PACKET_TIME,
            body: pran::level_body(pran.level, pran.exp),
        },
        rand::random(),
    )];

    match grew {
        pran::Growth::MustEvolve => {
            // Once, not on every kill: the pran is at a wall and will stay
            // there until somebody evolves it.
            if !session.pran_told_to_evolve {
                session.pran_told_to_evolve = true;
                // The original's own words. Text a player reads is content
                // rather than code: the client is Portuguese, the game is,
                // and the sentence exists in the source already -- inventing
                // an English one would be both a translation and a guess.
                frames.push(encode_client_message(
                    client_id,
                    "A sua pran precisa evoluir para ganhar exp.",
                ));
            }
            return frames;
        }
        pran::Growth::Grew { levels } if levels > 0 => {
            info!(name = %pran.name, level = pran.level, "the pran grew");
            session.pran_told_to_evolve = false;
            let described = frame::encode(
                &Message {
                    sender: dialog::FIXED_INDEX,
                    opcode: pran::OP_WORLD,
                    time: PACKET_TIME,
                    body: pran::world_body(pran),
                },
                rand::random(),
            );
            frames.push(encode_client_message(client_id, "Sua pran subiu de nível."));
            frames.push(described);
        }
        pran::Growth::Grew { .. } => {}
    }
    frames
}
/// The evolution quest, which is what actually changes a companion's shape.
///
/// Levelling carries the form and stops at 4, 19 and 49. What lifts it is a
/// quest -- 406 at the first wall and 407 at the second, both on the NPC that
/// hands prans out in the first place, and named in the original's own
/// comments. Nothing else in that source ever writes a class of 62, 63 or 64,
/// so these two are the only way a pran has ever changed.
///
/// There are no quests here to hang it on, so the quest option stands in for
/// the chain the same way it does for the character's promotion. A pran at a
/// wall is answered first, because that is the more specific thing to be
/// asking for while standing in front of the pran NPC with a pran on.
fn evolve_the_pran(state: &State, session: &mut Session) -> Option<Vec<Vec<u8>>> {
    let client_id = session.client_id;
    let at = session.pran_out?;

    let account = session.account.as_mut()?;
    let pran = account.prans.get_mut(at)?;
    let grown = match pran::evolve(pran) {
        Ok(grown) => grown,
        // Not a pran matter after all: let the character's promotion answer.
        Err(pran::NotYet::NotAtAWall) => return None,
        Err(why) => {
            return Some(vec![encode_client_message(client_id, why.message())]);
        }
    };
    session.dirty = true;
    info!(name = %pran.name, class = grown.class, stone = grown.stone, "the pran evolved");

    // The stone changes in two places, and the second is the one worth
    // saying out loud: the item the *player* is wearing becomes the new
    // stone as well. Changing only the pran's copy leaves the owner holding
    // the stone of a form their companion no longer is.
    let character = session.character.as_mut()?;
    let mut worn = character.items.get(inventory::EQUIP, pran::STONE_SLOT).cloned()?;
    worn.index = grown.stone;
    worn.appearance = grown.stone;
    let _ = character.items.put(worn.clone());

    // The exact four the quest sends, in the exact order:
    //
    //     SendEffect(0);
    //     SendPranToWorld(0);
    //     SendPranSpawn(0);
    //     SendRefreshItemSlot(EQUIP_TYPE, 10, Character.Equip[10], False);
    //
    // Note that this is `ToWorld` and *then* `Spawn`, which is the opposite
    // of the order the same two go out in on arrival. The original is not
    // consistent about it and this follows each path as it is: describing a
    // companion that is changing shape before redrawing it is not the same
    // act as drawing one that has just appeared, and guessing which order a
    // client wants has cost this project an evening already.
    let mut frames = vec![encode_menu_close()];
    if grown.clears_the_glow {
        frames.push(encode_effect(client_id, EFFECT_NONE));
    }
    session.pran_body = None;

    let account = session.account.as_ref()?;
    let pran = account.prans.get(at)?;
    frames.push(frame::encode(
        &Message {
            sender: dialog::FIXED_INDEX,
            opcode: pran::OP_WORLD,
            time: PACKET_TIME,
            body: pran::world_body(pran),
        },
        rand::random(),
    ));

    if pran.has_body() {
        if let Some(pran_id) = pran_client_id(client_id) {
            let owner = session.character.as_ref()?;
            let at = neighbour_spot(
                (owner.x as f32, owner.y as f32),
                rand::random::<usize>() % NEIGHBOUR_SPOTS,
            );
            let speed = stats::of(owner, &state.items, &Effects::none()).speed_move;
            frames.push(encode_pran_spawn(pran, owner, pran_id, at, speed));
            session.pran_body = Some(pran_id);
        }
    }

    frames.push(encode_refresh_item(inventory::EQUIP, pran::STONE_SLOT, &worn, false));
    frames.push(encode_client_message(client_id, "Sua pran evoluiu."));
    Some(frames)
}
/// Whatever the worn summon stone should be showing right now.
///
/// Called wherever equipment slot ten can have changed: entering the world,
/// and any move that touches it. The original does the same thing in the same
/// two places -- `Mob/Player.pas:5190` on arrival, and the move handler for
/// everything after (`PacketHandlers.pas:6573`).
///
/// # Hatching
///
/// A stone with no pran bound to it gets one. That part is ours: on the
/// original a pran comes from one of three quests -- 39 fire, 40 water, 41
/// air -- and `FinishQuest` is the only thing in the source that ever makes
/// one. There are no quests here yet, so the stone stands in for the chain,
/// and it hatches fire because the element is the quest's choice and fire is
/// no more arbitrary than the other two. When quests land, this is the
/// paragraph to delete: the numbers themselves are already the original's.
///
/// Nothing checks that the stone suits the pran. `GetPranClassStoneItem` says
/// which stone a class belongs in, but the original does not consult it here
/// -- it matches the stone's `Identific` and nothing else -- so neither does
/// this.
fn pran_frames(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let client_id = session.client_id;
    let worn = session
        .character
        .as_ref()
        .and_then(|c| c.items.get(inventory::EQUIP, pran::STONE_SLOT))
        .filter(|item| !item.is_empty())
        .cloned();

    // Nothing worn, so nothing to send. Arriving with an empty slot is not the
    // same as taking a stone off: the original only looks at slot ten on
    // arrival `if Equip[10].Identific > 0`, and a player who has never had a
    // pran should not be sent a packet about one. Clearing is the caller's to
    // do, and only where a stone has just left.
    let Some(stone) = worn else {
        session.pran_out = None;
        return Vec::new();
    };
    let is_stone = state
        .items
        .get(stone.index as usize)
        .is_some_and(|def| pran::is_stone(def.item_type()));
    if !is_stone {
        return Vec::new();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let Some(account) = session.account.as_mut() else {
        return Vec::new();
    };

    // A stone with no identific names no pran and can never be bound to one:
    // `belongs_to` is false for a pran whose `item_id` is zero, so hatching
    // against it would hatch again on the next look, and again after that.
    if stone.identific == 0 {
        debug!(stone = stone.index, "a summon stone with nothing to identify it");
        return Vec::new();
    }

    let at = match account.prans.iter().position(|p| p.belongs_to(&stone)) {
        Some(at) => at,
        None => {
            // Which element is written on the stone rather than chosen here:
            // the three quest rewards are items 100, 101 and 102, and that is
            // the whole of the mapping. A stone that is not one of the three
            // carries a pran that already exists and hatches nothing.
            let Some(element) = pran::element_of_quest_stone(stone.index) else {
                debug!(
                    stone = stone.index,
                    "a summon stone that no quest hands out, so nothing to hatch"
                );
                return Vec::new();
            };
            let hatched = pran::Pran::hatch(element, &stone, now);
            info!(
                stone = stone.index,
                identific = stone.identific,
                class = hatched.class,
                "a pran hatched"
            );
            account.prans.push(hatched);
            session.dirty = true;
            account.prans.len() - 1
        }
    };

    session.pran_out = Some(at);
    let pran = &account.prans[at];

    // Built here, sent last. The original spawns the body and *then* describes
    // it -- `SendPranSpawn(n); SendPranToWorld(n);`, in that order, in both of
    // the two places it does this: on arrival (`Mob/Player.pas:5190`) and in
    // the move handler for everything after. We had the two the other way
    // round, which is the kind of difference this protocol has punished before.
    let described = frame::encode(
        &Message {
            sender: dialog::FIXED_INDEX,
            opcode: pran::OP_WORLD,
            time: PACKET_TIME,
            body: pran::world_body(pran),
        },
        rand::random(),
    );

    // And its level, which nothing else carries.
    //
    // The description above says everything about a pran except how old it
    // is, and the client will not work that out from the experience -- it
    // shows level 1 until it is told otherwise, and draws the panel for a
    // level 1. A companion with a grown body and a hatchling's panel is what
    // that looks like, and it looked like that on every login: the original
    // only ever sends this on a gain, so a pran that comes out already grown
    // is a case it never had to answer.
    let aged = frame::encode(
        &Message {
            sender: dialog::FIXED_INDEX,
            opcode: pran::OP_LEVEL,
            time: PACKET_TIME,
            body: pran::level_body(pran.level, pran.exp),
        },
        rand::random(),
    );
    let mut frames = Vec::with_capacity(4);

    // The first form of each element has no body at all: it is an effect on
    // the player, one per element, and that is the whole of how it shows.
    // Every form after it is a companion of its own, standing beside the
    // player under an id from the pran range.
    if !pran.has_body() {
        session.pran_body = None;
        if let Some(element) = pran.element() {
            frames.push(encode_effect(client_id, element.fairy_effect()));
        }
        frames.push(described);
        frames.push(aged);
        return frames;
    }

    let Some(pran_id) = pran_client_id(client_id) else {
        warn!(client_id, "no companion id left for this connection");
        return frames;
    };
    let pran = pran.clone();
    let Some(owner) = session.character.as_ref() else {
        return frames;
    };
    let at = neighbour_spot(
        (owner.x as f32, owner.y as f32),
        rand::random::<usize>() % NEIGHBOUR_SPOTS,
    );
    let speed_move = stats::of(owner, &state.items, &Effects::none()).speed_move;
    frames.push(encode_pran_spawn(&pran, owner, pran_id, at, speed_move));
    frames.push(described);
    frames.push(aged);
    session.pran_body = Some(pran_id);
    frames
}

/// Takes the companion away again.
///
/// Which of the two ways depends on how it was shown, and that is the one
/// place this does not follow the original. `SendPranUnspawn` chooses by
/// *level*: under four it sends the effect away, otherwise it removes a body
/// (`Mob/Player.pas:3846`). But `SendPranSpawn`, twenty lines above it,
/// chooses by *class*. The two disagree for any pran whose class has a body
/// while its level is still under four -- it is drawn as a companion and
/// dismissed as an effect, and the body stays on the field with nothing left
/// to remove it.
///
/// So this undoes what was actually done. Summoning records the id it drew
/// under, and dismissing removes exactly that or clears the effect.
fn dismiss_pran(session: &mut Session) -> Vec<Vec<u8>> {
    session.pran_out = None;
    match session.pran_body.take() {
        Some(pran_id) => vec![encode_remove_mob(pran_id as u16, 0)],
        None => vec![encode_effect(session.client_id, EFFECT_NONE)],
    }
}

/// What the original sends to take a fairy off a player again.
const EFFECT_NONE: u32 = 0;
/// `TSendCreatePranPacket` (`0x349`), which the original's own comment calls
/// "PlayerSpam" because it is the player spawn laid out again, field for
/// field. So it is encoded with the same offsets, and the three that differ
/// are the ones filled in here: the build is three bytes rather than four,
/// the companion's own client id follows it, and the title is not a title but
/// "Pran do <owner>".
fn encode_pran_spawn(
    pran: &pran::Pran,
    owner: &Character,
    pran_id: u32,
    at: (f32, f32),
    speed_move: u32,
) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    write_fixed_str(&mut body[off::NAME..off::NAME + 16], &pran.name);

    // Its own eight equipment slots, by item id, and the first of them is what
    // the client draws it as -- the same field that carries the model in the
    // player spawn this packet is a copy of. A pran wears its own summon stone
    // there. Without it the client falls back to a bare human body, which is
    // exactly what turned up on the field: a half-height naked copy of the
    // player, correctly labelled "Pran do <owner>".
    for (slot, index) in pran.equipment.iter().enumerate() {
        put16(&mut body, off::EQUIP + slot * 2, *index);
    }

    body[off::POSITION_X..off::POSITION_X + 4].copy_from_slice(&at.0.to_le_bytes());
    body[off::POSITION_Y..off::POSITION_Y + 4].copy_from_slice(&at.1.to_le_bytes());

    put32(&mut body, off::MAX_HP, pran.max_hp);
    put32(&mut body, off::MAX_MP, pran.max_mp);
    put32(&mut body, off::CUR_HP, pran.hp);
    put32(&mut body, off::CUR_MP, pran.mp);

    body[off::UNK0] = SPAWN_UNK0;
    // It keeps up with whoever it belongs to, so it walks at their speed.
    body[off::SPEED_MOVE] = speed_move.min(u8::MAX as u32) as u8;
    body[off::SPAWN_TYPE] = SPAWN_NORMAL;

    // Three bytes, not the character's four: the fourth is where the
    // companion's client id begins.
    body[off::SIZES] = pran.width;
    body[off::SIZES + 1] = pran.chest;
    body[off::SIZES + 2] = pran.leg;
    put16(&mut body, off::PRAN_CLIENT_ID, pran_id as u16);

    let title = format!("Pran do {}", owner.name);
    write_fixed_str(
        &mut body[npc_offset::TITLE..npc_offset::TITLE + npc_offset::TITLE_MAX],
        &title,
    );

    put16(&mut body, off::GUILD_AND_NATION, owner.nation << 12);
    put16(&mut body, off::EFFECTS + 2, SPAWN_EFFECT_1);

    // The header carries the companion's id and not its owner's, which is
    // what makes the client draw a second body instead of moving the first.
    frame::encode(
        &Message { sender: pran_id as u16, opcode: pran::OP_SPAWN, time: 0, body },
        rand::random(),
    )
}

/// The client id a player's companion is drawn under.
///
/// The original takes the first free slot of a thousand-wide array. This
/// derives it from the owner instead, which needs no allocator and cannot
/// leak one: a connection owns exactly one, and player ids are unique while
/// they are connected. `None` past the end of the range, which is a server
/// holding more players than the original could.
fn pran_client_id(client_id: u16) -> Option<u32> {
    let id = pran::IDS.start() + client_id as u32 - 1;
    pran::IDS.contains(&id).then_some(id)
}

/// Where a companion is put down.
///
/// `SetCurrentNeighbors` keeps nine spots around every player and the
/// original picks one at random to stand its pran on. They are barely apart
/// -- half a unit out, growing by a tenth every second spot, alternating
/// which side of the player they fall on.
fn neighbour_spot(at: (f32, f32), which: usize) -> (f32, f32) {
    let offset = 0.5 + (which / 2) as f32 * 0.1;
    if which % 2 == 0 {
        (at.0 - offset, at.1 - offset)
    } else {
        (at.0 + offset, at.1 + offset)
    }
}

/// How many of them there are (`Neighbors: Array [0 .. 8]`).
const NEIGHBOUR_SPOTS: usize = 9;
/// The effect number the client plays when a character gains a level
/// (`AddLevel` sends `SendEffect(1)`).
const EFFECT_LEVEL_UP: u32 = 1;


/// The six basic skills every class is born knowing, and the marker the record
/// carries for a learned one (`SetPlayerSkills` writes `2`).
const BASIC_SKILL_COUNT: usize = 6;
const BASIC_SKILL_LEARNED: u16 = 2;

/// `Tp131` (`0x131`): zero except for one field of all ones.
fn encode_enter_131() -> Vec<u8> {
    let mut body = Vec::with_capacity(ENTER_131_SIZE - MIN_FRAME);
    body.extend_from_slice(&ENTER_131_MARKER.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, ENTER_131_SIZE);
    frame::encode(&Message { sender: 0, opcode: OP_ENTER_131, time: 0, body }, rand::random())
}

/// `TSignalData` (`Data/Packets.pas:31`): a header and one DWORD. 16 bytes.
fn encode_signal(opcode: u16, client_id: u16, time: u32, data: u32) -> Vec<u8> {
    frame::encode(
        &Message { sender: client_id, opcode, time, body: data.to_le_bytes().to_vec() },
        rand::random(),
    )
}

/// `TSendToWorldPacket` (`Data/Packets.pas:452`): the whole character.
fn encode_send_to_world(
    account: &Account,
    character: &Character,
    client_id: u16,
    time: u32,
    skills: &SkillTable,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + CHARACTER_SIZE);
    body.extend_from_slice(&account.id.to_le_bytes());
    body.extend_from_slice(&encode_character(character, client_id, &account.prans, skills));

    debug_assert_eq!(body.len() + MIN_FRAME, SEND_TO_WORLD_SIZE);
    frame::encode(
        &Message { sender: SEND_TO_WORLD_INDEX, opcode: OP_SEND_TO_WORLD, time, body },
        rand::random(),
    )
}

/// `TCharacter`. Only the fields the client needs to build the character;
/// the rest (inventory, skills, quests, titles) stays zeroed for now.
fn encode_character(
    character: &Character,
    client_id: u16,
    prans: &[pran::Pran],
    skills: &SkillTable,
) -> Vec<u8> {
    use character_offset as off;
    let mut out = vec![0u8; CHARACTER_SIZE];

    let put32 = |out: &mut Vec<u8>, at: usize, v: u32| {
        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put16 = |out: &mut Vec<u8>, at: usize, v: u16| {
        out[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };

    put32(&mut out, off::CLIENT_ID, client_id as u32);
    put32(&mut out, off::FIRST_LOGIN, 0);
    put32(&mut out, off::CHAR_INDEX, character.slot as u32 + 1);
    write_fixed_str(&mut out[off::NAME..off::NAME + 16], &character.name);
    out[off::NATION] = character.nation as u8;
    out[off::CLASS_INFO] = character.class_info() as u8;

    // What the companions are called. Nothing else tells the client, and a
    // client that does not know asks the player to name one that already has
    // a name -- then refuses to let it out of the chest until it is answered,
    // which it cannot be.
    for (slot, pran) in prans.iter().take(2).enumerate() {
        let at = off::PRAN_NAMES + slot * off::PRAN_NAME_SIZE;
        write_fixed_str(&mut out[at..at + off::PRAN_NAME_SIZE], &pran.name);
    }

    for (i, value) in character.attributes.iter().enumerate() {
        put16(&mut out, off::ATTRIBUTES + i * 2, *value);
    }
    out[off::SIZES..off::SIZES + 4].copy_from_slice(&character.sizes);

    // Provisional health and mana, just so the character is not born dead.
    // The real formulas depend on tables we have not read yet.
    let hp = 100 + character.level as u32 * 10;
    let mp = 50 + character.level as u32 * 5;
    put32(&mut out, off::MAX_HP, hp);
    put32(&mut out, off::CUR_HP, hp);
    put32(&mut out, off::MAX_MP, mp);
    put32(&mut out, off::CUR_MP, mp);

    out[off::EXP..off::EXP + 8].copy_from_slice(&character.exp.to_le_bytes());
    // Same convention as the character list: the client adds 1.
    put16(&mut out, off::LEVEL, character.level.saturating_sub(1));

    // The unspent skill points, which the skill window reads from here.
    put16(&mut out, off::SKILL_POINT, character.skill_points);

    // Everything carried, at the slot it occupies. Written before the
    // appearance below, because slots 0 and 1 of the equipment are the body
    // and the hair rather than real items, and those have to win.
    for item in character.items.iter() {
        let base = match item.container {
            inventory::EQUIP => off::EQUIP,
            inventory::BAG => off::INVENTORY,
            _ => continue,
        };
        let at = base + item.slot as usize * off::ITEM_SIZE;
        if at + off::ITEM_SIZE <= out.len() {
            write_item(&mut out[at..at + off::ITEM_SIZE], item);
        }
    }

    // Equip[0] is the class and Equip[1] the hair, in each item's `Index`.
    put16(&mut out, off::EQUIP, character.class_index);
    put16(&mut out, off::EQUIP + off::ITEM_SIZE, character.hair);

    // The skills it knows and the icons on its bar. The client reads both
    // straight out of this record, so an empty bar here is an empty bar on
    // screen -- and the skill list is built rather than copied, for the
    // reason spelled out on `set_player_skills`.
    out[off::SKILL_LIST..off::SKILL_LIST + SKILL_LIST_BYTES]
        .copy_from_slice(&set_player_skills(character, skills));
    for (i, slot) in character.item_bar.iter().enumerate() {
        put32(&mut out, off::ITEM_BAR + i * 4, *slot);
    }

    out[off::GOLD..off::GOLD + 8].copy_from_slice(&character.gold.to_le_bytes());
    put32(&mut out, off::LOCATION, 0);

    out
}

/// `TRequestLoginPacket` (`Data/Packets.pas:200`), 1096 bytes no total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLogin {
    pub account_id: u32,
    pub username: String,
    pub version: u16,
    pub token: String,
}

impl RequestLogin {
    /// Offsets inside the body (the 12-byte header is already gone).
    const ACCOUNT_ID: usize = 0;
    const USERNAME: usize = 4;
    const VERSION: usize = 54;
    const TOKEN: usize = 60;
    /// Body as declared in the Delphi record: 1096 minus the header.
    pub const BODY_SIZE: usize = 1084;
    /// What the client **actually** sends: 100 bytes total, 88 of body.
    /// The 992 `Null_1` bytes at the end of the record are buffer size and
    /// never reach the wire, so the token field arrives truncated. No loss:
    /// the original server does not validate the token here either, that is
    /// server, no `0x81`.
    pub const MIN_BODY: usize = Self::VERSION + 2;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_BODY {
            return None;
        }
        let token_end = (Self::TOKEN + 32).min(body.len());
        let token = if token_end > Self::TOKEN {
            read_fixed_str(&body[Self::TOKEN..token_end])
        } else {
            String::new()
        };
        Some(Self {
            account_id: u32::from_le_bytes(
                body[Self::ACCOUNT_ID..Self::ACCOUNT_ID + 4].try_into().ok()?,
            ),
            username: read_fixed_str(&body[Self::USERNAME..Self::USERNAME + 32]),
            version: u16::from_le_bytes(body[Self::VERSION..Self::VERSION + 2].try_into().ok()?),
            token,
        })
    }

    /// Builds the body the way the client does. Used by the tests.
    pub fn to_body(&self) -> Vec<u8> {
        let mut body = vec![0u8; Self::BODY_SIZE];
        body[Self::ACCOUNT_ID..Self::ACCOUNT_ID + 4]
            .copy_from_slice(&self.account_id.to_le_bytes());
        write_fixed_str(&mut body[Self::USERNAME..Self::USERNAME + 32], &self.username);
        body[Self::VERSION..Self::VERSION + 2].copy_from_slice(&self.version.to_le_bytes());
        write_fixed_str(&mut body[Self::TOKEN..Self::TOKEN + 32], &self.token);
        body
    }
}

/// `TSendToCharListPacket` (`Data/Packets.pas:233`): account id, two zeroed
/// two zeroed fields and three character entries.
pub fn encode_char_list(account: &Account, client_id: u16, time: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(CHAR_LIST_SIZE - MIN_FRAME);
    body.extend_from_slice(&account.id.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // Unk
    body.extend_from_slice(&0u32.to_le_bytes()); // NotUse

    for slot in 0..MAX_CHARACTERS {
        let character = account.characters.iter().find(|c| c.slot == slot);
        body.extend_from_slice(&encode_char_list_entry(character));
    }

    debug_assert_eq!(body.len() + MIN_FRAME, CHAR_LIST_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CHAR_LIST, time, body },
        rand::random(),
    )
}

/// `TCharacterListData` (`Data/Packets.pas:215`), 104 bytes: the entry on the
/// selection screen, far smaller than the world's `TCharacter`. An empty
/// entry is all zeroes.
fn encode_char_list_entry(character: Option<&Character>) -> [u8; CHAR_ENTRY_SIZE] {
    let mut out = [0u8; CHAR_ENTRY_SIZE];
    let Some(character) = character else {
        return out;
    };

    write_fixed_str(&mut out[0..16], &character.name);
    out[16..18].copy_from_slice(&character.nation.to_le_bytes());
    out[18..20].copy_from_slice(&character.class_info().to_le_bytes());
    out[20..24].copy_from_slice(&character.sizes);

    // The selection screen dresses the character the same way the world
    // does, from the appearance of each slot; the first two are then
    // overwritten with the body and the hair.
    for slot in WORN_SLOTS {
        let at = 24 + slot as usize * 2;
        out[at..at + 2].copy_from_slice(&worn_appearance(character, slot).to_le_bytes());
    }
    out[24..26].copy_from_slice(&character.class_index.to_le_bytes());
    out[26..28].copy_from_slice(&character.hair.to_le_bytes());

    // Refine[7] is hardcoded to 15 as in the original, where the weapon's real
    // is commented out there (`Mob/Player.pas:3038`).
    out[40 + 7] = 15;

    for (i, value) in character.attributes.iter().enumerate() {
        let at = 52 + i * 2;
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    // The client adds 1 to what it gets, so the server sends level minus one.
    out[64..66].copy_from_slice(&character.level.saturating_sub(1).to_le_bytes());
    out[72..80].copy_from_slice(&character.exp.to_le_bytes());
    out[80..88].copy_from_slice(&character.gold.to_le_bytes());

    out
}

/// Hex of the leading bytes, to check offsets against the real client.
fn hex_dump(body: &[u8]) -> String {
    body.iter().take(112).map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn read_fixed_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0x00 || b == 0xCC).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect::<String>().trim().to_string()
}

fn write_fixed_str(dest: &mut [u8], value: &str) {
    for (slot, byte) in dest.iter_mut().zip(value.chars().map(|c| c as u8)) {
        *slot = byte;
    }
}

mod items;
pub(crate) use items::*;
mod social;
pub(crate) use social::*;

#[cfg(test)]
mod tests;
