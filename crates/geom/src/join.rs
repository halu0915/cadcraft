//! The outer corner where a band of constant half width turns: shared by the renderer, which
//! fills it, and the extents, which must contain it.

use crate::Vec2;

/// Miters longer than this many half widths are bevelled (PDF's default miter limit).
pub const MITER_LIMIT: f64 = 10.0;

/// The outside of a turn at a vertex `c`, for a band of half width `hw`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OuterJoin {
    /// Outer band corner of the arriving segment.
    pub o0: Vec2,
    /// Outer band corner of the leaving segment.
    pub o1: Vec2,
    /// Unit bisector of the two outer normals, pointing outwards.
    pub bis: Vec2,
    /// Distance from the vertex to the miter tip, in half widths (infinite for a reversal).
    pub miter: f64,
}

impl OuterJoin {
    /// The miter tip when it is within [`MITER_LIMIT`]; past it the corner is bevelled
    /// (`o0`–`o1`) and has no tip.
    pub fn tip(&self, c: Vec2, hw: f64) -> Option<Vec2> {
        (self.miter <= MITER_LIMIT).then(|| c + self.bis * (hw * self.miter))
    }
}

/// The outer corner at `c` between a band arriving in unit direction `d0` and one leaving in
/// `d1`; `None` when it goes straight on.
pub fn outer_join(c: Vec2, d0: Vec2, d1: Vec2, hw: f64) -> Option<OuterJoin> {
    let turn = d0.cross(d1);
    if turn.abs() <= 1e-12 && d0.dot(d1) > 0.0 {
        return None;
    }
    // The outer side of the turn: right of a left turn, left of a right turn.
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let n0 = d0.perp() * (hw * side);
    let n1 = d1.perp() * (hw * side);
    let bis = (n0 + n1).normalized();
    let cos_half = if hw > 0.0 { bis.dot(n0) / hw } else { 0.0 };
    let miter = if cos_half > 1e-9 { 1.0 / cos_half } else { f64::INFINITY };
    Some(OuterJoin { o0: c + n0, o1: c + n1, bis, miter })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_angle_tip_is_the_square_corner() {
        let j = outer_join(Vec2::ZERO, Vec2::X, Vec2::new(0.0, 1.0), 0.5).unwrap();
        let tip = j.tip(Vec2::ZERO, 0.5).unwrap();
        assert!((tip - Vec2::new(0.5, -0.5)).len() < 1e-12, "{tip:?}");
    }

    #[test]
    fn straight_has_no_join_and_reversal_has_no_tip() {
        assert!(outer_join(Vec2::ZERO, Vec2::X, Vec2::X, 1.0).is_none());
        let back = outer_join(Vec2::ZERO, Vec2::X, -Vec2::X, 1.0).unwrap();
        assert!(back.tip(Vec2::ZERO, 1.0).is_none());
    }
}
