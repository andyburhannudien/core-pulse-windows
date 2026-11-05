//! Icon vocabulary for the UI.
//!
//! One glyph set, drawn from geometric shapes that exist in Segoe UI and
//! Consolas. Earlier revisions mixed emoji, dingbats and box-drawing characters,
//! which produced visibly different weights and baselines — exactly what makes a
//! technical UI look improvised.
//!
//! Rules:
//! - Status is always a circle: `●` (on/good), `○` (off/none).
//! - Change direction is always a triangle: `▲` up, `▼` down.
//! - Affirmation is `✓` / `✕`, never emoji.
//! - Unknown is the em dash `—`; pending is the ellipsis `…`.
//! - Navigation uses one geometric family at one size, never mixed with text.

/// Filled status dot, for places where a painted circle is not practical
/// (tooltips, log lines). Prefer `widgets::dot` inside dense tables.
#[allow(dead_code)]
pub const DOT_ON: &str = "●";
#[allow(dead_code)]
pub const DOT_OFF: &str = "○";

/// Raised / increased.
pub const UP: &str = "▲";
/// Cleared / decreased.
pub const DOWN: &str = "▼";

pub const CHECK: &str = "✓";
pub const CROSS: &str = "✕";

/// Unknown value. Never a zero.
pub const DASH: &str = "—";
/// Pending value (not read yet). Distinct from `DASH` on purpose: a dash means
/// the source refused or is absent, while pending means nobody has looked yet.
pub const PENDING: &str = "…";

/// Approximate / derived value marker.
pub const APPROX: &str = "≈";
/// Third-party value marker.
pub const THIRD_PARTY: &str = "◇";
/// Bullet for prose lists.
pub const BULLET: &str = "•";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_are_single_characters() {
        // A multi-character "icon" would look different from every other one.
        for glyph in [
            DOT_ON,
            DOT_OFF,
            UP,
            DOWN,
            CHECK,
            CROSS,
            DASH,
            PENDING,
            APPROX,
            THIRD_PARTY,
            BULLET,
        ] {
            assert_eq!(
                glyph.chars().count(),
                1,
                "icon {glyph:?} must be exactly one character"
            );
        }
    }

    #[test]
    fn dash_and_pending_are_distinct() {
        assert_ne!(DASH, PENDING);
        assert_ne!(DOT_ON, DOT_OFF);
        assert_ne!(UP, DOWN);
    }
}
