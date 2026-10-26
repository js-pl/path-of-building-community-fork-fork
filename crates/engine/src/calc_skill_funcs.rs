//! The skills' own offence functions (`initialFunc`, `preDamageFunc`, ...
//! of `Data/Skills/*.lua`), ported by hand per granted effect id.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::calc_offence::Off;
use crate::calc_perform::Perform;

pub type Result<T> = std::result::Result<T, String>;

impl<'a> Perform<'a> {
    /// Runs the function `name` of the granted effect `id`
    pub(crate) fn skill_func(&mut self, _o: &mut Off, id: &str, name: &str) -> Result<()> {
        match (id, name) {
            _ => Err(format!("not ported: {name} of {id}")),
        }
    }
}
