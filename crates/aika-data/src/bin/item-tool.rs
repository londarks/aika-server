//! Looks items up in the server's `ItemList.bin`.
//!
//! ```text
//! item-tool count  <ItemList.bin>
//! item-tool show   <ItemList.bin> <id>
//! item-tool find   <ItemList.bin> <text>
//! ```
//!
//! Useful on its own to check a price or a level requirement, and useful as a
//! sanity check on the parser: if the names come out readable and the prices
//! look like money, the record offsets are right.

use aika_data::itemlist::ItemList;
use std::process::ExitCode;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match refs.as_slice() {
        ["count", path] => count(path),
        ["show", path, id] => match id.parse() {
            Ok(id) => show(path, id),
            Err(_) => Err("the id must be a number".into()),
        },
        ["find", path, text] => find(path, text),
        _ => {
            eprintln!("usage:");
            eprintln!("  item-tool count <ItemList.bin>");
            eprintln!("  item-tool show  <ItemList.bin> <id>");
            eprintln!("  item-tool find  <ItemList.bin> <text>");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load(path: &str) -> Result<ItemList, Box<dyn std::error::Error>> {
    Ok(ItemList::decode(&std::fs::read(path)?)?)
}

fn count(path: &str) -> Fallible {
    let list = load(path)?;
    let defined = list.defined().count();
    println!("{} ids in the table, {defined} of them defined", list.len());

    let sellable = list.defined().filter(|(_, i)| i.price_gold() > 0).count();
    println!("{sellable} have a gold price");
    Ok(())
}

fn show(path: &str, id: usize) -> Fallible {
    let list = load(path)?;
    let Some(item) = list.get(id) else {
        println!("id {id} is not defined");
        return Ok(());
    };

    println!("[{id}] {}", item.name());
    println!("  english      {}", item.name_english());
    if !item.description().is_empty() {
        println!("  description  {}", item.description());
    }
    println!("  type {}  rarity {}  trade {}", item.item_type(), item.rarity(), item.trade_kind());
    println!("  level {}  max level {}", item.level(), item.max_level());
    println!(
        "  price   gold {}  honor {}  medal {}  sells for {}",
        item.price_gold(),
        item.price_honor(),
        item.price_medal(),
        item.sell_price()
    );
    println!(
        "  stats   atk {} def {} matk {} mdef {} hp {} mp {}",
        item.attack(),
        item.defense(),
        item.magic_attack(),
        item.magic_defense(),
        item.hp(),
        item.mp()
    );
    println!("  stacks {}  durability {}", item.can_group(), item.durability());
    Ok(())
}

fn find(path: &str, text: &str) -> Fallible {
    let list = load(path)?;
    let needle = text.to_lowercase();

    let mut found = 0;
    for (id, item) in list.defined() {
        let name = item.name();
        if name.to_lowercase().contains(&needle)
            || item.name_english().to_lowercase().contains(&needle)
        {
            println!("[{id:>6}] {:<40} gold {}", name, item.price_gold());
            found += 1;
            if found == 40 {
                println!("...");
                break;
            }
        }
    }
    if found == 0 {
        println!("nothing matches {text:?}");
    }
    Ok(())
}
