//! The sunburst, against the properties a reader relies on without saying so.
//!
//! Three of them carry the view: a ring's segments cover the circle exactly
//! once, a child sits inside the angular span of its parent, and a point on
//! screen maps back to the arc that was drawn there. Break any one and the
//! picture still looks like a sunburst while meaning something else.

use std::f64::consts::TAU;

use spacetrace_scan_core::{ImportedNode, SizeBasis, Tree};
use spacetrace_treemap::sunburst::{sunburst, Sunburst, SunburstOptions};

fn file(name: &str, size: u64) -> ImportedNode {
    ImportedNode::file(name, size, size)
}

fn dir(name: &str, children: Vec<ImportedNode>) -> ImportedNode {
    let mut node = ImportedNode::dir(name);
    node.children = children;
    node
}

fn tree_of(children: Vec<ImportedNode>) -> Tree {
    Tree::from_nested(std::path::PathBuf::from("/root"), dir("root", children))
}

fn options() -> SunburstOptions {
    SunburstOptions {
        basis: SizeBasis::Logical,
        ..SunburstOptions::default()
    }
}

fn laid_out(tree: &Tree) -> Sunburst {
    sunburst(tree, tree.root(), &options())
}

/// The first ring has to account for the whole circle: a gap is a folder the
/// reader cannot see, and an overlap is one drawn over another.
#[test]
fn one_ring_covers_the_circle_exactly_once() {
    let tree = tree_of(vec![
        file("a.bin", 500),
        file("b.bin", 300),
        file("c.bin", 200),
    ]);
    let out = laid_out(&tree);

    let ring: Vec<_> = out.arcs().iter().filter(|a| a.depth == 1).collect();
    assert_eq!(ring.len(), 3);

    let covered: f64 = ring.iter().map(|a| a.sweep).sum();
    assert!(
        (covered - TAU).abs() < 1e-9,
        "covered {covered}, not a full turn"
    );

    // Adjacent and in order, with no gap between one and the next.
    let mut sorted = ring.clone();
    sorted.sort_by(|x, y| x.start.total_cmp(&y.start));
    assert!(
        (sorted[0].start).abs() < 1e-9,
        "the first begins at twelve o'clock"
    );
    for pair in sorted.windows(2) {
        let end = pair[0].start + pair[0].sweep;
        assert!(
            (end - pair[1].start).abs() < 1e-9,
            "a gap of {} between segments",
            pair[1].start - end
        );
    }
}

/// An entry twice the size of another gets twice the angle. The claim the
/// whole view makes.
#[test]
fn the_angle_is_the_share() {
    let tree = tree_of(vec![file("big.bin", 600), file("small.bin", 200)]);
    let out = laid_out(&tree);

    let by_name = |name: &str| {
        out.arcs()
            .iter()
            .find(|a| tree.name(a.node) == name)
            .copied()
            .unwrap()
    };
    let big = by_name("big.bin");
    let small = by_name("small.bin");
    assert!((big.sweep / small.sweep - 3.0).abs() < 1e-9);
    assert!((big.sweep - TAU * 0.75).abs() < 1e-9);
    // Largest first, starting at the top.
    assert!(big.start < small.start);
}

/// A child's span inside its parent's is what makes the rings line up. Without
/// it the picture is a stack of unrelated pie charts.
#[test]
fn every_child_sits_within_its_parent() {
    let tree = tree_of(vec![
        dir(
            "left",
            vec![file("x.bin", 400), file("y.bin", 100), file("z.bin", 100)],
        ),
        dir("right", vec![file("q.bin", 400)]),
    ]);
    let out = laid_out(&tree);

    for arc in out.arcs().iter().filter(|a| a.depth == 2) {
        let parent = tree.node(arc.node).parent;
        let parent_arc = out
            .arcs()
            .iter()
            .find(|a| a.node == parent)
            .expect("the parent must be laid out too");

        assert!(
            arc.start >= parent_arc.start - 1e-9
                && arc.start + arc.sweep <= parent_arc.start + parent_arc.sweep + 1e-9,
            "{} spans {}..{} outside its parent's {}..{}",
            tree.name(arc.node),
            arc.start,
            arc.start + arc.sweep,
            parent_arc.start,
            parent_arc.start + parent_arc.sweep
        );
        assert_eq!(
            arc.inner_radius, parent_arc.outer_radius,
            "a ring must begin where the one inside it ends"
        );
    }
}

/// A child's sweep is its share of its parent, not of the whole disc. Two
/// folders of very different sizes each fill their own segment completely.
#[test]
fn a_childs_share_is_of_its_parent() {
    let tree = tree_of(vec![
        dir("huge", vec![file("a.bin", 900)]),
        dir("tiny", vec![file("b.bin", 100)]),
    ]);
    let out = laid_out(&tree);

    for parent_name in ["huge", "tiny"] {
        let parent = out
            .arcs()
            .iter()
            .find(|a| tree.name(a.node) == parent_name)
            .unwrap();
        let only_child = out
            .arcs()
            .iter()
            .find(|a| {
                a.depth == 2
                    && a.start >= parent.start - 1e-9
                    && a.start < parent.start + parent.sweep
            })
            .unwrap();
        assert!(
            (only_child.sweep - parent.sweep).abs() < 1e-9,
            "{parent_name}'s only child should fill it"
        );
    }
}

/// Pointing at something has to select the thing that was drawn there. Tested
/// by aiming at each arc's own middle, which is the only point guaranteed to
/// be inside it however thin it is.
#[test]
fn a_point_finds_the_arc_that_was_drawn_there() {
    let tree = tree_of(vec![
        dir("one", vec![file("a.bin", 300), file("b.bin", 200)]),
        dir("two", vec![file("c.bin", 400)]),
        file("loose.bin", 100),
    ]);
    let out = laid_out(&tree);

    for arc in out.arcs() {
        if arc.depth == 0 {
            continue;
        }
        let angle = arc.start + arc.sweep / 2.0;
        let radius = (arc.inner_radius + arc.outer_radius) / 2.0;
        // The layout's convention, inverted: clockwise from twelve o'clock.
        let (dx, dy) = (radius * angle.sin(), -radius * angle.cos());

        let found = out.hit(dx, dy).expect("the middle of an arc must hit it");
        assert_eq!(
            found.node,
            arc.node,
            "aimed at {} and got {}",
            tree.name(arc.node),
            tree.name(found.node)
        );
    }
}

/// The seam at twelve o'clock is where an angle wraps from `TAU` back to zero,
/// and it is exactly where a naive implementation returns nothing.
#[test]
fn the_point_just_before_twelve_oclock_hits_the_last_segment() {
    let tree = tree_of(vec![file("a.bin", 500), file("b.bin", 500)]);
    let out = laid_out(&tree);

    let radius = out.arcs()[1].inner_radius + 1.0;
    // A hair anticlockwise of straight up: inside the last segment, which ends
    // at exactly a full turn.
    let angle = TAU - 1e-4;
    let hit = out
        .hit(radius * angle.sin(), -radius * angle.cos())
        .expect("just short of the seam is still inside the last segment");
    assert_eq!(hit.depth, 1);

    // And nothing at all beyond the outermost ring.
    assert!(out.hit(out.radius() + 10.0, 0.0).is_none());
    // The hole in the middle belongs to the root's disc, not to a ring.
    assert_eq!(out.hit(0.0, 0.0).map(|a| a.depth), Some(0));
}

/// An arc too thin to aim at is not drawn, and the parent says there is more
/// inside — the same contract the treemap's area limit has.
#[test]
fn what_is_too_thin_to_see_is_not_drawn_and_is_admitted() {
    let mut children = vec![file("whale.bin", 10_000_000)];
    for i in 0..40 {
        children.push(file(&format!("speck{i}.bin"), 1));
    }
    let tree = tree_of(children);
    let out = laid_out(&tree);

    let ring: Vec<_> = out.arcs().iter().filter(|a| a.depth == 1).collect();
    assert!(ring.len() < 41, "the specks should not all be drawn");
    assert!(
        out.arcs()[0].truncated,
        "the root has to admit that something was left out"
    );
}

/// Depth is a hard stop, because the rings have to fit on a screen.
#[test]
fn the_ring_count_can_be_capped() {
    let tree = tree_of(vec![dir(
        "a",
        vec![dir("b", vec![dir("c", vec![file("deep.bin", 100)])])],
    )]);
    let capped = sunburst(
        &tree,
        tree.root(),
        &SunburstOptions {
            max_depth: Some(2),
            ..options()
        },
    );
    assert_eq!(capped.arcs().iter().map(|a| a.depth).max(), Some(2));
    assert!(
        capped.arcs().iter().any(|a| a.depth == 2 && a.truncated),
        "the outermost drawn ring has to say there is more"
    );
}

/// An empty folder produces a middle disc and nothing else, rather than a
/// division by zero or a full ring of nothing.
#[test]
fn an_empty_tree_is_just_the_middle() {
    let tree = tree_of(vec![]);
    let out = laid_out(&tree);
    assert_eq!(out.len(), 1);
    assert_eq!(out.arcs()[0].depth, 0);
    assert!(!out.arcs()[0].truncated);
    assert!(out.hit(1000.0, 1000.0).is_none());
}

/// Which measure the angles follow is a parameter, not a default (invariant
/// #6). A sparse file claims far more than it holds, and the two bases have to
/// produce visibly different pictures.
#[test]
fn the_basis_changes_the_picture() {
    let mut sparse = ImportedNode::file("sparse.img", 1_000_000, 1_000);
    sparse.mtime = 0;
    let dense = ImportedNode::file("dense.bin", 1_000, 1_000);
    let tree = tree_of(vec![sparse, dense]);

    let logical = sunburst(&tree, tree.root(), &options());
    let on_disk = sunburst(
        &tree,
        tree.root(),
        &SunburstOptions {
            basis: SizeBasis::OnDisk,
            ..options()
        },
    );

    let sweep_of = |out: &Sunburst, name: &str| {
        out.arcs()
            .iter()
            .find(|a| tree.name(a.node) == name)
            .map(|a| a.sweep)
            .unwrap()
    };
    assert!(
        sweep_of(&logical, "sparse.img") > TAU * 0.9,
        "by length the sparse file should dominate"
    );
    assert!(
        (sweep_of(&on_disk, "sparse.img") - TAU / 2.0).abs() < 1e-9,
        "by blocks the two are equal"
    );
}
