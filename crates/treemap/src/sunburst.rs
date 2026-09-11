//! The same tree as rings instead of rectangles.
//!
//! A treemap spends its area on the biggest entries, which is what makes it
//! good at "what is taking up the space" and bad at the other question: how
//! deep the thing is, and what is nested inside what. A sunburst answers that
//! one. Each ring is a level, each arc is an entry, and an arc's sweep is its
//! share of its parent — so the shape of the tree is the picture rather than
//! something to be inferred from nesting.
//!
//! **Angles, not areas.** In a treemap an entry's *area* is its size; here its
//! *angle* is. The area of an arc grows with its radius, so a small folder
//! sitting far out looks larger than a big one near the middle, and a reader
//! who compares arcs by how much ink they take will be wrong every time. This
//! is a real limitation of the view rather than of this implementation — it is
//! why the treemap stays the default and this is the second opinion.
//!
//! **The share is of the parent, not of the root.** A child's sweep is its
//! fraction of its parent's sweep, which is what makes a ring's segments line
//! up under the one above. It also means a deep entry's angle says nothing
//! about its share of the whole disk, only of the folder it is in.
//!
//! **What is too thin to see is not drawn.** An arc narrower than
//! [`SunburstOptions::min_sweep`] cannot carry a border, let alone be aimed at
//! with a pointer, and subdividing it costs work for nothing. The parent is
//! marked `truncated` so the view can say there is more inside, the same way
//! the treemap does with its area limit.

use std::f64::consts::TAU;

use spacetrace_scan_core::{NodeId, SizeBasis, Tree};

/// One laid-out ring segment.
///
/// Angles are radians clockwise from twelve o'clock, which is where a reader
/// starts: the largest child of the root begins at the top. Radii are in the
/// same units the caller gave, so a renderer can place them without scaling.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Arc {
    pub node: NodeId,
    /// Where the segment begins, radians from twelve o'clock.
    pub start: f64,
    /// How much of the circle it covers, in radians.
    pub sweep: f64,
    pub inner_radius: f64,
    pub outer_radius: f64,
    /// Rings below the layout root; the root's own disc is 0.
    pub depth: u16,
    /// True when subdivision stopped here even though the entry has children.
    pub truncated: bool,
}

impl Arc {
    /// Whether a point in polar coordinates falls inside this segment.
    ///
    /// `angle` is normalised by the caller; see [`Sunburst::hit`].
    fn contains(&self, radius: f64, angle: f64) -> bool {
        radius >= self.inner_radius
            && radius < self.outer_radius
            && angle >= self.start
            && angle < self.start + self.sweep
    }
}

#[derive(Debug, Clone)]
pub struct SunburstOptions {
    /// How thick each ring is, in the caller's units.
    pub ring: f64,
    /// The empty disc in the middle, where the root's own label goes.
    ///
    /// Not zero, and not only for the label: the innermost ring's segments
    /// would otherwise converge to a point, which makes the first level —
    /// the one being read most — the hardest to aim at.
    pub hole: f64,
    /// Arcs narrower than this are not drawn and are not descended into.
    ///
    /// In radians. The default is about a third of a degree, which at a
    /// 300-unit radius is roughly two units of arc: thin enough to be honest
    /// about what is on screen, wide enough that a pointer can land on it.
    pub min_sweep: f64,
    /// Never go further out than this many rings.
    pub max_depth: Option<u16>,
    /// Which measurement the angles are proportional to. The same question as
    /// the treemap's, with the same answer: it decides the whole picture.
    pub basis: SizeBasis,
}

impl Default for SunburstOptions {
    fn default() -> Self {
        SunburstOptions {
            ring: 34.0,
            hole: 46.0,
            min_sweep: TAU / 1080.0,
            max_depth: None,
            basis: SizeBasis::Logical,
        }
    }
}

/// A laid-out sunburst: arcs in an arena, parents before children.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Sunburst {
    arcs: Vec<Arc>,
}

impl Sunburst {
    pub fn arcs(&self) -> &[Arc] {
        &self.arcs
    }

    pub fn len(&self) -> usize {
        self.arcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.arcs.is_empty()
    }

    /// How far out the drawing reaches, so a caller can size its canvas.
    pub fn radius(&self) -> f64 {
        self.arcs.iter().map(|a| a.outer_radius).fold(0.0, f64::max)
    }

    /// The arc under a point, given relative to the centre.
    ///
    /// The deepest match wins, which is the one drawn on top: rings do not
    /// overlap, so at most one arc per ring can contain a point and the
    /// outermost is the most specific answer.
    pub fn hit(&self, dx: f64, dy: f64) -> Option<&Arc> {
        let radius = dx.hypot(dy);
        // Clockwise from twelve o'clock, to match the layout. `atan2(dx, -dy)`
        // rather than the usual `atan2(dy, dx)` does exactly that rotation and
        // flip in one step; the `rem_euclid` puts the result in `0..TAU` so a
        // point just left of noon is `TAU - ε` and not a small negative.
        let angle = dx.atan2(-dy).rem_euclid(TAU);
        self.arcs
            .iter()
            .filter(|arc| arc.contains(radius, angle))
            .max_by_key(|arc| arc.depth)
    }
}

/// Lay a subtree out as rings.
///
/// Returns arcs in draw order: a parent always precedes its children, so
/// painting the slice in order puts outer rings over inner ones.
pub fn sunburst(tree: &Tree, root: NodeId, opts: &SunburstOptions) -> Sunburst {
    let mut arcs = Vec::new();
    if opts.ring <= 0.0 || opts.hole < 0.0 {
        return Sunburst { arcs };
    }

    // The root is the middle disc, covering the whole circle. It carries a
    // sweep of a full turn so that a child's share can be computed the same
    // way at every level, including the first.
    arcs.push(Arc {
        node: root,
        start: 0.0,
        sweep: TAU,
        inner_radius: 0.0,
        outer_radius: opts.hole,
        depth: 0,
        truncated: tree.node(root).children_len > 0,
    });

    // Breadth-first, so one ring is finished before the next begins and the
    // arcs of a ring sit together in the slice.
    let mut queue = std::collections::VecDeque::from([0usize]);

    while let Some(index) = queue.pop_front() {
        let (node, start, sweep, depth, outer) = {
            let a = &arcs[index];
            (a.node, a.start, a.sweep, a.depth, a.outer_radius)
        };

        if opts.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        if sweep < opts.min_sweep {
            continue;
        }

        let children = sorted_children(tree, node, opts.basis);
        if children.is_empty() {
            arcs[index].truncated = false;
            continue;
        }
        let total: f64 = children.iter().map(|(_, size)| size).sum();
        if total <= 0.0 {
            continue;
        }

        let first = arcs.len();
        let mut cursor = start;
        let mut hidden = false;
        for (child, size) in children {
            // Share of the parent, which is what makes a ring line up under
            // the one inside it.
            let child_sweep = sweep * (size / total);
            if child_sweep < opts.min_sweep {
                // The children are sorted largest first, so once one is too
                // thin every one after it is too. Stopping here rather than
                // continuing saves the rest of the loop and is why the flag
                // can be set once.
                hidden = true;
                break;
            }
            arcs.push(Arc {
                node: child,
                start: cursor,
                sweep: child_sweep,
                inner_radius: outer,
                outer_radius: outer + opts.ring,
                depth: depth + 1,
                truncated: tree.node(child).children_len > 0,
            });
            cursor += child_sweep;
        }

        // Nothing was wide enough: the parent keeps its mark, because there
        // genuinely is more inside than is drawn.
        if arcs.len() == first {
            continue;
        }
        arcs[index].truncated = hidden;

        for child_index in first..arcs.len() {
            queue.push_back(child_index);
        }
    }

    Sunburst { arcs }
}

/// Children with a non-zero size, largest first.
///
/// The same rule the treemap uses, for the same reason: an entry contributing
/// nothing under the chosen measure would be a segment of no width, and
/// keeping it produces a degenerate arc rather than information.
fn sorted_children(tree: &Tree, node: NodeId, basis: SizeBasis) -> Vec<(NodeId, f64)> {
    let mut children: Vec<(NodeId, f64)> = tree
        .children(node)
        .map(|c| (c, tree.node(c).measure(basis) as f64))
        .filter(|(_, size)| *size > 0.0)
        .collect();
    children.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    children
}
