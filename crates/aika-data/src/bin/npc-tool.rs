//! Reads the `.npc` files the server places in the world.
//!
//! ```text
//! npc-tool list  <Data/NPCs>
//! npc-tool show  <Data/NPCs> <id>
//! npc-tool near  <Data/NPCs> <x> <y> [radius]
//! ```
//!
//! `near` is the one that earns its keep: give it the coordinates a player is
//! standing on and it says who should be on screen.

use aika_data::npc::{Npc, NpcSet};
use std::process::ExitCode;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match refs.as_slice() {
        ["list", dir] => list(dir),
        ["show", dir, id] => match id.parse() {
            Ok(id) => show(dir, id),
            Err(_) => Err("the id must be a number".into()),
        },
        ["near", dir, x, y] => near(dir, x, y, "60"),
        ["near", dir, x, y, radius] => near(dir, x, y, radius),
        _ => {
            eprintln!("usage:");
            eprintln!("  npc-tool list <Data/NPCs>");
            eprintln!("  npc-tool show <Data/NPCs> <id>");
            eprintln!("  npc-tool near <Data/NPCs> <x> <y> [radius]");
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

fn load(dir: &str) -> Result<NpcSet, Box<dyn std::error::Error>> {
    let set = NpcSet::load_dir(dir)?;
    if set.is_empty() {
        return Err(format!("no .npc file in {dir}").into());
    }
    Ok(set)
}

fn list(dir: &str) -> Fallible {
    let set = load(dir)?;
    for npc in set.iter() {
        println!(
            "[{:>4}] {:<24} {:<28} at ({:>6.0}, {:>6.0})  menu {:?}",
            npc.id, npc.label, npc.title, npc.x, npc.y, npc.options
        );
    }
    println!("\n{} npcs", set.len());

    let stale = set.iter().filter(|n| n.stale_id.is_some()).count();
    if stale > 0 {
        println!("{stale} carry an id from the file they were copied from");
    }
    for (file, why) in &set.rejected {
        println!("  skipped {file}: {why}");
    }
    Ok(())
}

fn show(dir: &str, id: u16) -> Fallible {
    let set = load(dir)?;
    let Some(npc) = set.get(id) else {
        println!("no npc with id {id}");
        return Ok(());
    };

    println!("[{}] {} - {}", npc.id, npc.label, npc.title);
    match npc.name_index {
        Some(index) => println!("  name       string {index} in the client's table"),
        None => println!("  name       not an index into the string table"),
    }
    println!("  position   ({:.1}, {:.1})  rotation {}", npc.x, npc.y, npc.rotation);
    println!("  menu       {:?}", npc.options);
    if let Some(stale) = npc.stale_id {
        println!("  note       the record says {stale}; the file name says {}", npc.id);
    }
    Ok(())
}

fn near(dir: &str, x: &str, y: &str, radius: &str) -> Fallible {
    let set = load(dir)?;
    let (x, y): (f32, f32) = (x.parse()?, y.parse()?);
    let radius: f32 = radius.parse()?;

    let mut found: Vec<(f32, &Npc)> = set
        .iter()
        .map(|npc| (((npc.x - x).powi(2) + (npc.y - y).powi(2)).sqrt(), npc))
        .filter(|(distance, _)| *distance <= radius)
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));

    for (distance, npc) in &found {
        println!("{distance:>7.1}  [{:>4}] {:<24} {}", npc.id, npc.label, npc.title);
    }
    println!("\n{} within {radius} of ({x}, {y})", found.len());
    Ok(())
}
