//! Talking to a non-player character.
//!
//! One packet does everything: `0x30F` carries the NPC's id and the option
//! that was clicked, and zero means "I just clicked the NPC itself". The
//! server answers with the menu, and every later `0x30F` on the same NPC is a
//! choice from it.
//!
//! The menu is not invented here: each `.npc` file carries up to ten option
//! numbers, in the order they should appear. What each number *means* is the
//! dispatch in `PacketHandlers.pas:4264`, and what each one *reads as* is a
//! table of 65 strings. The original ships that table as `NPCOptionsText.bin`
//! in Portuguese; ours is in code and in English, so the server does not
//! depend on a file from the original pack to put words on a menu.
//!
//! ```text
//! client -> 0x30F  { npc: u32, option: u32, extra: u32 }
//! server -> 0x110  signal: a menu follows
//! server -> 0x10E  which NPC it belongs to
//! server -> 0x112  one per entry { option: u32, _: u32, text[64], colour: u32 }
//! server -> 0x10F  signal: close the menu
//! ```

use aika_data::npc::Npc;

/// `TOpenNPCPacket` (`Data/Packets.pas:1030`).
pub const OP_OPEN_NPC: u16 = 0x30F;
/// The client asking to close the window it has open.
pub const OP_CLOSE_NPC_OPTION: u16 = 0x348;

/// Server to client.
pub const OP_MENU_BEGIN: u16 = 0x110;
pub const OP_MENU_OWNER: u16 = 0x10E;
pub const OP_MENU_ENTRY: u16 = 0x112;
pub const OP_MENU_CLOSE: u16 = 0x10F;

/// `TShowOptionsPacket`: header, two DWORDs, a 64 byte name and a colour.
pub const MENU_ENTRY_SIZE: usize = 12 + 4 + 4 + 64 + 4;
pub const MENU_TEXT_MAX: usize = 64;

/// The index the original stamps on packets it does not address to a player.
pub const FIXED_INDEX: u16 = 0x7535;

/// How close a player has to stand to talk. The original checks a radius of
/// 10 and closes the window when it fails (`PacketHandlers.pas:3262`).
pub const TALK_RANGE: f32 = 10.0;

/// Options the menu can carry, from the dispatch in `PacketHandlers.pas`.
/// Only the ones the server can actually answer are listed; the rest still
/// appear on the menu and are refused with a message when clicked.
pub mod option {
    /// Not an option: the client clicked the NPC and wants the menu.
    pub const OPEN: u32 = 0;
    pub const TALK: u32 = 1;
    pub const QUESTS: u32 = 2;
    pub const TELEPORT: u32 = 3;
    pub const ENTER_CASTLE: u32 = 4;
    pub const SHOP: u32 = 5;
    pub const SKILLS: u32 = 6;
    pub const STORAGE: u32 = 7;
    pub const CLOSE: u32 = 8;
    pub const CREATE_GUILD: u32 = 10;
    pub const SIGN_IN_CASTLE: u32 = 12;
}

/// Colours the original gives certain entries (`NPCHandlers.pas:176`). ARGB,
/// and the client paints the line with them.
pub const COLOUR_DEFAULT: u32 = 0xFFFF_FFFF;
pub const COLOUR_CLOSE: u32 = 0xFFEB_5A5A;
pub const COLOUR_HEADING: u32 = 0xFF7F_C1F4;

/// The 65 menu entries, in English.
///
/// These are a translation of `NPCOptionsText.bin`, entry by entry, with the
/// original's Portuguese kept beside each one so the two can be checked
/// against each other. Four of them are not in the file at all: `TLoad`
/// overwrites entries 47, 59, 60 and 64 in memory after reading it
/// (`Functions/Load.pas:3099`), and those overwrites are what the player
/// actually sees.
///
/// Keeping our own copy means one less file from the pack to carry, but it
/// has to be a translation and not an invention: an earlier version of this
/// table was guessed past entry 13, which labelled *Repair* as *Buy* and sent
/// somebody clicking the wrong line of a merchant's menu. Index 0 is unused —
/// the file numbers its options from 1.
///
/// `Nothing_N` in the file means an unused slot, and those stay empty here:
/// `entries` drops anything without text rather than drawing a blank line.
const OPTION_TEXT: [&str; 66] = [
    "",                                // index 0 is unused
    "Talk",                            // Conversa
    "Quest",                           // Missao
    "Teleport",                        // Teleportar
    "Enter the castle",                // Entrar no Castelo
    "Shop",                            // Loja
    "Skills",                          // Habilidades
    "Storage",                         // Armazem
    "Close",                           // Fechar
    "Sign up for the war",             // Cadastrar para Guerra
    "Create a guild",                  // Criar Guild
    "Guild storage",                   // Armazem da Guild
    "Sign up for the castle",          // Cadastrar para o castelo
    "Pran station",                    // Central da Pran
    "",                                // Nothing_2
    "Craft",                           // Manofaturar
    "Refine",                          // Refinar
    "Enchant an item",                 // Encantar Item
    "Change appearance",               // Trocar Aparencia
    "Break an item down",              // Reduzir Item
    "Timat",                           // Timat, a place
    "",                                // Nothing_3
    "",                                // Nothing_4
    "",                                // Nothing_5
    "",                                // Nothing_6
    "Save your location",              // Salvar Localizacao
    "Enter the dungeon",               // Entrar na Dungeon
    "",                                // Nothing_7
    "",                                // Nothing_8
    "Pran hairdresser",                // Cabeleireiro da Pran
    "",                                // Nothing_9
    "Repair",                          // Consertar
    "Repair everything",               // Consertar Tudo
    "Dismantle",                       // Desmontar
    "",                                // Nothing_10
    "Blessing",                        // Bencao
    "",                                // Nothing_11
    "Agross",                          // Agross, a place
    "Return from Agross",              // Retorno Agross
    "Upgrade a relic",                 // Aprimorar Reliquia
    "",                                // Nothing_12
    "Current standing",                // Situacao Atual
    "Switch alliance",                 // Alternar Alianca
    "Return to your nation",           // Retornar Nacao
    "Enchant a mount",                 // Encantar Montaria
    "",                                // Nothing_13
    "Guild skills",                    // Habilidade Guild
    "Teleport to your saved location",  // patched over Nothing_14 at load
    "Auction house",                   // Leilao
    "Karak airship",                   // Karak Aereo
    "",                                // Nothing_15
    "",                                // Nothing_16
    "Teleport to Leopold",             // Teleporte Leopold
    "",                                // Nothing_17
    "Change nation",                   // Trocar Nacao
    "",                                // Nothing_18
    "",                                // Nothing_19
    "Teleport to Karena",              // Teleporte Karena
    "",                                // Nothing_20
    "Teleport to Balavan",             // patched over Nothing_21 at load
    "Teleport to Panzabil",            // patched over Nothing_22 at load
    "",                                // Nothing_23
    "Enchant a Pran",                  // Encantar Pran
    "",                                // Nothing_24
    "Perfect party blessing (6 members)",  // patched over Nothing_25 at load
    "Blessing (50k)",                  // Bencao 50k
];

/// The words a menu entry shows.
pub fn option_text(option: u8) -> &'static str {
    OPTION_TEXT.get(option as usize).copied().unwrap_or("")
}

/// The colour an entry is painted in.
pub fn option_colour(option: u8) -> u32 {
    match option as u32 {
        option::CLOSE => COLOUR_CLOSE,
        21 => COLOUR_HEADING,
        _ => COLOUR_DEFAULT,
    }
}

/// Which entries an NPC should actually show.
///
/// The file lists what the NPC was built with, but a merchant whose stock is
/// empty offering a shop is a window that opens onto nothing. Filtering here
/// keeps that decision in one place rather than at the moment of the click.
pub fn entries(npc: &Npc) -> Vec<u8> {
    npc.options
        .iter()
        .copied()
        .filter(|&o| o as u32 != option::SHOP || npc.sells())
        .filter(|&o| !option_text(o).is_empty())
        .collect()
}

/// What the client asked for. `option` is `OPEN` on the first click and the
/// chosen entry afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenNpc {
    pub npc: u32,
    pub option: u32,
    pub extra: u32,
}

impl OpenNpc {
    pub const BODY_SIZE: usize = 12;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            npc: u32::from_le_bytes(body[0..4].try_into().ok()?),
            option: u32::from_le_bytes(body[4..8].try_into().ok()?),
            extra: u32::from_le_bytes(body[8..12].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.npc.to_le_bytes());
        body.extend_from_slice(&self.option.to_le_bytes());
        body.extend_from_slice(&self.extra.to_le_bytes());
        body
    }

    /// Whether the original would leave the window open after this choice.
    /// Talking, quests and the menu heading keep it; everything else makes
    /// the client close and reopen (`PacketHandlers.pas:3382`).
    pub fn keeps_window_open(&self) -> bool {
        matches!(self.option, option::TALK | option::QUESTS | 21)
    }
}

/// The body of one `0x112`.
pub fn menu_entry_body(option: u8) -> Vec<u8> {
    let mut body = vec![0u8; MENU_ENTRY_SIZE - 12];
    body[0..4].copy_from_slice(&(option as u32).to_le_bytes());

    let text = option_text(option).as_bytes();
    let len = text.len().min(MENU_TEXT_MAX - 1);
    body[8..8 + len].copy_from_slice(&text[..len]);

    let colour = option_colour(option);
    let at = body.len() - 4;
    body[at..at + 4].copy_from_slice(&colour.to_le_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npc_with(options: Vec<u8>, sells: bool) -> Npc {
        let mut shop = [0u16; aika_data::npc::SHOP_SLOTS];
        if sells {
            shop[0] = 1000;
        }
        Npc {
            id: 2050,
            title: "Merchant".into(),
            label: "Thomas Henrikson".into(),
            name_index: Some(43),
            options,
            equip: [234, 234, 0, 0, 0, 0, 0, 0],
            sizes: [7, 119, 119, 3],
            shop,
            max_hp: 20000,
            cur_hp: 20000,
            max_mp: 20000,
            cur_mp: 0,
            x: 3468.4,
            y: 963.4,
            rotation: 0,
            speed_move: 0,
            stale_id: None,
        }
    }

    #[test]
    fn open_npc_body_roundtrip() {
        let original = OpenNpc { npc: 2050, option: option::SHOP, extra: 0 };
        assert_eq!(OpenNpc::parse(&original.to_body()), Some(original));
        assert_eq!(OpenNpc::parse(&[0u8; 4]), None, "a short body is not a request");
    }

    /// The real merchant's menu, from `[2050] Thomas Henrikson.npc`.
    #[test]
    fn a_merchant_shows_the_entries_from_its_file() {
        let npc = npc_with(vec![1, 2, 31, 32, 5, 8], true);
        assert_eq!(entries(&npc), vec![1, 2, 31, 32, 5, 8]);
    }

    /// A shop entry on an NPC with nothing to sell opens an empty window.
    #[test]
    fn an_empty_shop_is_not_offered() {
        let npc = npc_with(vec![1, 2, 5, 8], false);
        assert_eq!(entries(&npc), vec![1, 2, 8], "the shop entry survived");
    }

    /// The file has option numbers we have no words for. Showing a blank line
    /// is worse than showing nothing.
    #[test]
    fn entries_without_text_are_dropped() {
        let npc = npc_with(vec![1, 14, 8], false);
        assert_eq!(entries(&npc), vec![1, 8]);
    }

    /// The table is a translation of `NPCOptionsText.bin`, not a guess.
    /// These are the entries a shipped `.npc` file actually references, and
    /// getting one wrong sends a player clicking the wrong line: 31 was
    /// labelled "Buy" once, and it is Repair.
    #[test]
    fn the_option_text_matches_the_file_it_translates() {
        assert_eq!(option_text(1), "Talk");
        assert_eq!(option_text(2), "Quest");
        assert_eq!(option_text(5), "Shop");
        assert_eq!(option_text(6), "Skills");
        assert_eq!(option_text(8), "Close");
        assert_eq!(option_text(25), "Save your location");
        assert_eq!(option_text(31), "Repair");
        assert_eq!(option_text(32), "Repair everything");
        assert_eq!(option_text(50), "");

        // the four the original patches in after reading the file
        assert_eq!(option_text(47), "Teleport to your saved location");
        assert_eq!(option_text(59), "Teleport to Balavan");
        assert_eq!(option_text(60), "Teleport to Panzabil");
        assert_eq!(option_text(64), "Perfect party blessing (6 members)");

        // and nothing past the end of the table panics
        assert_eq!(option_text(255), "");
    }

    #[test]
    fn a_menu_entry_carries_its_number_text_and_colour() {
        let body = menu_entry_body(5);
        assert_eq!(body.len(), MENU_ENTRY_SIZE - 12);
        assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 5);

        let text: String = body[8..].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
        assert_eq!(text, "Shop");

        let at = body.len() - 4;
        assert_eq!(u32::from_le_bytes(body[at..at + 4].try_into().unwrap()), COLOUR_DEFAULT);
    }

    #[test]
    fn the_close_entry_is_painted_differently() {
        let body = menu_entry_body(8);
        let at = body.len() - 4;
        assert_eq!(u32::from_le_bytes(body[at..at + 4].try_into().unwrap()), COLOUR_CLOSE);
    }

    /// Text longer than the field must not run past it, and must stay
    /// terminated so the client stops reading.
    #[test]
    fn long_text_is_cut_and_still_terminated() {
        let longest = OPTION_TEXT.iter().map(|t| t.len()).max().unwrap();
        assert!(longest < MENU_TEXT_MAX, "an entry no longer fits its field");

        let body = menu_entry_body(8);
        assert_eq!(body[8 + MENU_TEXT_MAX - 1], 0, "the text field must end in a NUL");
    }

    #[test]
    fn talking_keeps_the_window_open_and_a_shop_does_not() {
        let talk = OpenNpc { npc: 2050, option: option::TALK, extra: 0 };
        let shop = OpenNpc { npc: 2050, option: option::SHOP, extra: 0 };

        assert!(talk.keeps_window_open());
        assert!(!shop.keeps_window_open());
    }
}
