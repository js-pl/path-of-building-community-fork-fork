//! Enchanting an item, as `ItemsTab:EnchantDisplayItem` does
//! (`Classes/ItemsTab.lua`): the enchantment sources of
//! `data.enchantmentSource` (`Modules/Data.lua`) and an enchantment
//! line put in an enchantment slot.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::item::{Item, ModLine};

/// `data.enchantmentSource`: name and label
pub const ENCHANTMENT_SOURCES: [(&str, &str); 10] = [
    ("ENKINDLING", "Enkindling Orb"),
    ("INSTILLING", "Instilling Orb"),
    ("RUNESMITH", "Runecraft Bench"),
    ("HEIST", "Heist"),
    ("HARVEST", "Harvest"),
    ("DEDICATION", "Dedication to the Goddess"),
    ("ENDGAME", "Eternal Labyrinth"),
    ("MERCILESS", "Merciless Labyrinth"),
    ("CRUEL", "Cruel Labyrinth"),
    ("NORMAL", "Normal Labyrinth"),
];

fn crafted(line: &str) -> ModLine {
    let mut m = ModLine {
        line: line.to_string(),
        ..ModLine::default()
    };
    m.flags.crafted = true;
    m
}

/// `table.insert(t, pos, v)` as LuaJIT does it
fn insert_at<T>(t: &mut Vec<T>, pos: usize, v: T) -> Result<(), String> {
    if pos == 0 || pos > t.len() + 1 {
        return Err("not ported: an enchantment slot past the item's enchantments".into());
    }
    t.insert(pos - 1, v);
    Ok(())
}

/// The enchantment lines of `enchantItem`: `line` (two lines when
/// joined by "/") put in `slot` (1-based), or the slot's line removed;
/// the item is to be built and parsed again after
pub fn enchant(
    item: &mut Item,
    slot: usize,
    line: Option<&str>,
    remove: bool,
) -> Result<(), String> {
    let lines = &mut item.enchant_mod_lines;
    if remove {
        // `t_remove(t, pos)` removes nothing past the list's end
        if slot >= 1 && slot <= lines.len() {
            lines.remove(slot - 1);
        }
        return Ok(());
    }
    let Some(line) = line else { return Ok(()) };
    if let Some(c) = crate::pat::captures(line, "([^/]+)/([^/]+)") {
        *lines = vec![crafted(&c[0]), crafted(&c[1])];
        return Ok(());
    }
    if !item.can_have_two_enchants && lines.len() > 1 {
        lines.truncate(1);
    }
    if lines.len() >= slot {
        lines.remove(slot - 1);
    }
    insert_at(lines, slot, crafted(line))
}
