//! Diagnostic: dump every cached character's PCStats — the mapped stats and, more
//! importantly, the *unmapped* bits — so a per-character flag RH does not model
//! (e.g. Blood Ritual) can be found by comparing a character that has it with ones
//! that do not. Their orders are identical, so any bit/value that differs is a
//! candidate.
//!
//! Usage: cargo run --example pcstats_probe -- <account_data.json>

use realmhound_core::api::{decode_pcstats, get_unmapped_pcstats};
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    characters: Characters,
}

#[derive(Deserialize)]
struct Characters {
    #[serde(default)]
    characters: Vec<Char>,
}

#[derive(Deserialize)]
struct Char {
    char_id: i32,
    #[serde(default)]
    class_id: i32,
    #[serde(default)]
    crucible_active: bool,
    #[serde(default)]
    pc_stats_raw: String,
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: pcstats_probe <account_data.json>");
    let raw = std::fs::read_to_string(&path).expect("read account data");
    let file: File = serde_json::from_str(&raw).expect("parse account data");

    for c in &file.characters.characters {
        let mapped = decode_pcstats(&c.pc_stats_raw);
        let unmapped = get_unmapped_pcstats(&c.pc_stats_raw);
        match mapped {
            Some(stats) => println!(
                "char {} class {} crucible={} blob_len={} stats={:?}",
                c.char_id,
                c.class_id,
                c.crucible_active,
                c.pc_stats_raw.len(),
                stats
            ),
            None => println!(
                "char {} class {} crucible={} blob_len={} stats=<undecoded>",
                c.char_id,
                c.class_id,
                c.crucible_active,
                c.pc_stats_raw.len(),
            ),
        }
        let bits: Vec<String> = unmapped
            .iter()
            .map(|(bit, value)| format!("bit{bit}={value}"))
            .collect();
        println!("    unmapped: {}", bits.join(", "));
    }
}
