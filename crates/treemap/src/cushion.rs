//! Cushion shading.
//!
//! J. J. van Wijk and H. van de Wetering, "Cushion Treemaps: Visualization of
//! Hierarchical Information", Proc. IEEE Symposium on Information
//! Visualization (InfoVis '99), pp. 73–78, 1999 (doi 10.1109/INFVIS.1999.801860).
//! This is the look SequoiaView introduced and WinDirStat adopted.
//!
//! A flat treemap shows sizes and hides structure: two neighbours in the same
//! folder and two neighbours a dozen levels apart are drawn the same way, and
//! the only cue left is the thickness of the lines between them. A cushion map
//! turns the hierarchy into a surface instead. Every rectangle adds a parabolic
//! ridge over its own extent, in x and in y, so each tile becomes a bump and
//! each folder a larger bump the bumps of its contents sit on. Lit from one
//! side, the bottom of every valley is a seam between two tiles, and the depth
//! of the valley says how far apart in the tree they are.
//!
//! **The surface is four numbers per tile.** A parabola in x plus one in y is
//! `z = x2·x² + x1·x + y2·y² + y1·y + c`, the sum of parabolas is a parabola,
//! and the constant never reaches the shading because only the slope does. So a
//! tile's whole surface — its own ridge and every ancestor's — is its parent's
//! four coefficients plus its own ridge, worked out in the layout pass that
//! makes the rectangle, with no second walk over the tree.
//!
//! Two departures from the paper's listing:
//!
//! * **A ridge in both directions per tile.** The paper bends the surface only
//!   in the direction each level splits, because its layout is slice-and-dice
//!   and alternates. A squarified tile is cut out of a row and has no single
//!   split direction, so it gets both.
//! * **Screen coordinates.** The paper's y axis points up and its light sits
//!   "slightly offset to the right and to above", `l = [1, 2, 10]`. Layout space
//!   has y growing downward, so the same light is `[1, -2, 10]` here.

use crate::Rect;

/// The paper's `h`: ridge height as a fraction of a tile's width, before the
/// falloff. A tile at depth `d` gets `h·f^d`, as in the paper's driver routine.
pub const HEIGHT: f64 = 0.5;

/// How much lower each level's ridges are than its parent's: the paper's `f`.
///
/// 0.75 is the value the paper's driver routine uses. Below 1 the outer levels
/// dominate, so the large folders read first and the files inside them as
/// texture on top; at 1 every level is as strong as every other, the paper's
/// self-similar surface.
pub const FALLOFF: f64 = 0.75;

/// Ambient light, `Ia = 40` of 255 in the paper: the darkest a pixel gets.
pub const AMBIENT: f64 = 40.0 / 255.0;

/// Directional light, `Is = 215` of 255: added to [`AMBIENT`] where a surface
/// faces the light head-on, so the brightest pixel is exactly 1.
pub const DIFFUSE: f64 = 215.0 / 255.0;

/// Unit vector towards the light, `[1, -2, 10] / √105` in layout space.
///
/// Written out because `sqrt` is not `const`; a test checks it against the
/// direction it claims to be.
pub const LIGHT: [f64; 3] = [
    0.097_590_007_294_853_33,
    -0.195_180_014_589_706_66,
    0.975_900_072_948_533_2,
];

/// The height a tile at `depth` adds, as a fraction of its width.
///
/// The layout root adds nothing, as in the paper (`if t.parent ≠ nil`). Here
/// that root is whatever folder the map is zoomed into, so a ridge on it would
/// be one bump spanning the whole view — every zoom target lit as a dome, light
/// on one side of the window and dark on the other, saying nothing.
pub fn ridge_height(depth: u16) -> f64 {
    if depth == 0 {
        return 0.0;
    }
    HEIGHT * FALLOFF.powi(i32::from(depth))
}

/// The parabolic surface over one tile: `z = x2·x² + x1·x + y2·y² + y1·y`.
///
/// The paper's `s[X,2]`, `s[X,1]`, `s[Y,2]`, `s[Y,1]`, in that order.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cushion {
    pub x2: f64,
    pub x1: f64,
    pub y2: f64,
    pub y1: f64,
}

impl Cushion {
    /// No ridge at all: what the surface is before the root.
    pub const FLAT: Cushion = Cushion {
        x2: 0.0,
        x1: 0.0,
        y2: 0.0,
        y1: 0.0,
    };

    /// This surface with one more ridge over `rect`, `height` times its size
    /// high at the centre.
    ///
    /// The paper's `AddRidge`, once per direction. The ridge in x is
    /// `4h(x - x₁)(x₂ - x)/(x₂ - x₁)`: zero on both edges, `h·(x₂ - x₁)` at the
    /// centre, and a slope of `±4h` at the edges whatever the width — which is
    /// why a small tile reads as the same shape as a large one.
    ///
    /// A side of zero width adds nothing in that direction rather than dividing
    /// by it. Such a tile covers no pixel, but its coefficients still cross to
    /// the window, and an infinity there serialises as `null`.
    pub fn with_ridge(self, rect: Rect, height: f64) -> Cushion {
        let mut out = self;
        if rect.w > 0.0 {
            let (lo, hi) = (rect.x, rect.x + rect.w);
            out.x1 += 4.0 * height * (hi + lo) / rect.w;
            out.x2 -= 4.0 * height / rect.w;
        }
        if rect.h > 0.0 {
            let (lo, hi) = (rect.y, rect.y + rect.h);
            out.y1 += 4.0 * height * (hi + lo) / rect.h;
            out.y2 -= 4.0 * height / rect.h;
        }
        out
    }

    /// Height of the surface at a point, up to a constant.
    ///
    /// Nothing draws with this — shading needs only the slope — but it is what
    /// "the peak is at the centre" means, so it is what the tests ask.
    pub fn height_at(&self, x: f64, y: f64) -> f64 {
        self.x2 * x * x + self.x1 * x + self.y2 * y * y + self.y1 * y
    }

    /// The surface normal at a point, not normalised: `[-∂z/∂x, -∂z/∂y, 1]`.
    pub fn normal_at(&self, x: f64, y: f64) -> [f64; 3] {
        [
            -(2.0 * self.x2 * x + self.x1),
            -(2.0 * self.y2 * y + self.y1),
            1.0,
        ]
    }

    /// Brightness at a point, as a factor for the tile's colour: the paper's
    /// `Ia + Is·max(0, cos α)`, scaled to `[AMBIENT, AMBIENT + DIFFUSE]`.
    ///
    /// The window evaluates this same expression per pixel; it cannot call it,
    /// because a few million calls across the bridge per frame is not a frame.
    /// This copy is the one the tests hold to account.
    pub fn intensity_at(&self, x: f64, y: f64) -> f64 {
        let [nx, ny, nz] = self.normal_at(x, y);
        let cos =
            (nx * LIGHT[0] + ny * LIGHT[1] + nz * LIGHT[2]) / (nx * nx + ny * ny + nz * nz).sqrt();
        AMBIENT + DIFFUSE * cos.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The highest point of one ridge pair is the middle of the rectangle, and
    /// it is a peak in every direction, not a saddle.
    #[test]
    fn a_single_rectangle_peaks_at_its_centre() {
        let rect = Rect::new(120.0, 40.0, 300.0, 80.0);
        let c = Cushion::FLAT.with_ridge(rect, HEIGHT);
        let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        let peak = c.height_at(cx, cy);

        let n = c.normal_at(cx, cy);
        assert!(
            close(n[0], 0.0) && close(n[1], 0.0),
            "flat at the top: {n:?}"
        );
        for (dx, dy) in [
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (7.0, -3.0),
        ] {
            assert!(
                c.height_at(cx + dx, cy + dy) < peak,
                "({dx}, {dy}) away from the centre is not lower"
            );
        }
    }

    /// What makes a ridge's shape independent of the tile's size: it rises by
    /// `h` times the width, so its edges always slope at `4h`.
    #[test]
    fn a_ridge_rises_by_its_height_times_the_width() {
        let rect = Rect::new(10.0, 0.0, 200.0, 50.0);
        let c = Cushion::FLAT.with_ridge(rect, 0.5);
        let rise_x = c.height_at(110.0, 0.0) - c.height_at(10.0, 0.0);
        assert!(close(rise_x, 0.5 * 200.0), "rise in x {rise_x}");
        let rise_y = c.height_at(0.0, 25.0) - c.height_at(0.0, 0.0);
        assert!(close(rise_y, 0.5 * 50.0), "rise in y {rise_y}");

        let left = c.normal_at(10.0, 25.0);
        let right = c.normal_at(210.0, 25.0);
        assert!(
            close(left[0], -2.0) && close(right[0], 2.0),
            "{left:?} {right:?}"
        );
    }

    /// Ridges add: a child's surface is its parent's with one more ridge on it,
    /// and adding them in either order lands on the same four numbers.
    #[test]
    fn ridges_add_independently_of_their_order() {
        let outer = Rect::new(0.0, 0.0, 400.0, 300.0);
        let inner = Rect::new(50.0, 60.0, 120.0, 90.0);
        let a = Cushion::FLAT
            .with_ridge(outer, 0.5)
            .with_ridge(inner, 0.375);
        let b = Cushion::FLAT
            .with_ridge(inner, 0.375)
            .with_ridge(outer, 0.5);
        for (p, q) in [(a.x2, b.x2), (a.x1, b.x1), (a.y2, b.y2), (a.y1, b.y1)] {
            assert!(close(p, q), "{a:?} against {b:?}");
        }
    }

    #[test]
    fn a_side_of_zero_width_adds_nothing_in_that_direction() {
        let c = Cushion::FLAT.with_ridge(Rect::new(5.0, 5.0, 0.0, 10.0), HEIGHT);
        assert_eq!((c.x2, c.x1), (0.0, 0.0));
        assert!(c.y2 < 0.0, "the other direction still gets its ridge");
        let none = Cushion::FLAT.with_ridge(Rect::new(5.0, 5.0, 0.0, 0.0), HEIGHT);
        assert_eq!(none, Cushion::FLAT);
    }

    #[test]
    fn the_root_is_flat_and_each_level_is_lower_by_the_falloff() {
        assert_eq!(ridge_height(0), 0.0);
        assert!(close(ridge_height(1), HEIGHT * FALLOFF));
        for depth in 1..40 {
            assert!(close(
                ridge_height(depth + 1),
                ridge_height(depth) * FALLOFF
            ));
        }
        // Deep enough to underflow `powi` would be a zero, not a NaN.
        assert!(ridge_height(u16::MAX).is_finite());
    }

    #[test]
    fn the_light_is_the_papers_direction_in_layout_space() {
        let len = (LIGHT[0] * LIGHT[0] + LIGHT[1] * LIGHT[1] + LIGHT[2] * LIGHT[2]).sqrt();
        assert!(close(len, 1.0), "not a unit vector: {len}");
        let scale = 105f64.sqrt();
        assert!(close(LIGHT[0] * scale, 1.0));
        assert!(close(LIGHT[1] * scale, -2.0), "above means negative y here");
        assert!(close(LIGHT[2] * scale, 10.0));
    }

    /// The face towards the light — right and up, on screen — is the bright one.
    #[test]
    fn the_upper_right_of_a_cushion_is_lit_and_the_lower_left_is_not() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let c = Cushion::FLAT.with_ridge(rect, HEIGHT);
        let upper_right = c.intensity_at(80.0, 20.0);
        let lower_left = c.intensity_at(20.0, 80.0);
        assert!(
            upper_right > lower_left + 0.2,
            "{upper_right} vs {lower_left}"
        );
        // A flat surface faces straight up and gets the light's z share.
        let flat = Cushion::FLAT.intensity_at(50.0, 50.0);
        assert!(close(flat, AMBIENT + DIFFUSE * LIGHT[2]));
    }

    /// Whatever the surface, the factor stays between ambient and full light.
    /// Steep stacks of thin, deep tiles are where an unclamped cosine would go
    /// negative, so they are what this sweeps.
    #[test]
    fn intensity_stays_between_ambient_and_full_light() {
        let mut c = Cushion::FLAT;
        let mut rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut seen_min = f64::INFINITY;
        let mut seen_max = f64::NEG_INFINITY;
        for depth in 1..30u16 {
            // Alternately thin in x and in y, drifting to the far corner.
            rect = if depth % 2 == 0 {
                Rect::new(rect.x + rect.w * 0.6, rect.y, rect.w * 0.4, rect.h)
            } else {
                Rect::new(rect.x, rect.y + rect.h * 0.7, rect.w, rect.h * 0.3)
            };
            c = c.with_ridge(rect, ridge_height(depth) * 4.0);
            for i in 0..=20 {
                for j in 0..=20 {
                    let x = rect.x + rect.w * f64::from(i) / 20.0;
                    let y = rect.y + rect.h * f64::from(j) / 20.0;
                    let v = c.intensity_at(x, y);
                    seen_min = seen_min.min(v);
                    seen_max = seen_max.max(v);
                    assert!(
                        (AMBIENT..=AMBIENT + DIFFUSE).contains(&v),
                        "{v} at ({x}, {y}), depth {depth}"
                    );
                }
            }
        }
        // And the sweep reached both ends, so the bound was exercised.
        assert!(close(seen_min, AMBIENT), "darkest seen {seen_min}");
        assert!(seen_max > 0.95, "brightest seen {seen_max}");
    }
}
