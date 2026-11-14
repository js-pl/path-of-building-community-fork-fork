//! Reordering a list by dragging one of its rows within it (`control` with
//! `sel` and `moveTo`): `ListControl`'s left button release on a mutable
//! list, which moves the selected row to before the row it is dropped on
//! and calls the list's `OnOrderChange`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::Result;
use serde_json::Value as J;

/// Where a drop moved a row
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Moved {
    /// `selIndex`, the row's old position (1-based)
    pub from: usize,
    /// `selDragIndex` after the move, its new position (1-based)
    pub to: usize,
}

/// The request's `moveTo`, if it is a move
pub(super) fn move_to(req: &J) -> Option<f64> {
    req["moveTo"].as_f64()
}

/// The drop of the selected row `sel` (1-based, a valid row) before row
/// `to`: `None` when it is dropped on itself, which leaves the list and
/// its selection as they are
pub(super) fn move_row<T>(list: &mut Vec<T>, sel: usize, to: f64) -> Result<Option<Moved>> {
    let to = to.min(list.len() as f64 + 1.0);
    if to < 1.0 || to.fract() != 0.0 {
        return Err(format!("no row {to}"));
    }
    let mut to = to as usize;
    if to == sel {
        return Ok(None);
    }
    let v = list.remove(sel - 1);
    if to > sel {
        to -= 1;
    }
    list.insert(to - 1, v);
    Ok(Some(Moved { from: sel, to }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_like_list_control() {
        let mut l = vec![1, 2, 3, 4];
        assert_eq!(move_row(&mut l, 1, 3.0), Ok(Some(Moved { from: 1, to: 2 })));
        assert_eq!(l, [2, 1, 3, 4]);
        assert_eq!(move_row(&mut l, 4, 1.0), Ok(Some(Moved { from: 4, to: 1 })));
        assert_eq!(l, [4, 2, 1, 3]);
        assert_eq!(move_row(&mut l, 2, 9.0), Ok(Some(Moved { from: 2, to: 4 })));
        assert_eq!(l, [4, 1, 3, 2]);
        assert_eq!(move_row(&mut l, 3, 3.0), Ok(None));
        // Dropped just below itself: moved to where it was
        assert_eq!(move_row(&mut l, 3, 4.0), Ok(Some(Moved { from: 3, to: 3 })));
        assert_eq!(l, [4, 1, 3, 2]);
    }
}
