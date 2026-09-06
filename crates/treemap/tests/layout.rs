//! Layout tests against real scanned trees.
//!
//! The properties that matter for a treemap are geometric, so they are checked
//! as properties — areas are proportional, tiles do not overlap, nothing
//! escapes its parent — rather than by pinning exact coordinates, which would
//! break on any harmless change to the packing.

use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, NodeId, ScanOptions, ScanProgress, Tree};
use spacetrace_treemap::{layout, LayoutOptions, Rect, Tile, TileTree};

/// A tree whose top level is deliberately lopsided, like a real disk.
fn fixture() -> (tempfile::TempDir, Tree) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("big")).unwrap();
    fs::create_dir(dir.path().join("big/inner")).unwrap();
    fs::create_dir(dir.path().join("medium")).unwrap();
    fs::create_dir(dir.path().join("small")).unwrap();
    fs::write(dir.path().join("big/a.bin"), vec![0u8; 600_000]).unwrap();
    fs::write(dir.path().join("big/inner/b.bin"), vec![0u8; 200_000]).unwrap();
    fs::write(dir.path().join("medium/c.bin"), vec![0u8; 150_000]).unwrap();
    fs::write(dir.path().join("small/d.bin"), vec![0u8; 20_000]).unwrap();
    fs::write(dir.path().join("loose.txt"), vec![0u8; 5_000]).unwrap();

    let (tree, _) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();
    (dir, tree)
}

fn full_canvas() -> Rect {
    Rect::new(0.0, 0.0, 1000.0, 700.0)
}

fn no_padding() -> LayoutOptions {
    LayoutOptions {
        min_area: 1.0,
        padding: 0.0,
        max_depth: None,
    }
}

/// Every tile must sit inside its parent, or nesting is a lie.
fn assert_nested(map: &TileTree, tile: &Tile) {
    for child in map.children_of(tile) {
        let (p, c) = (tile.rect, child.rect);
        let slack = 1e-6;
        assert!(
            c.x >= p.x - slack
                && c.y >= p.y - slack
                && c.x + c.w <= p.x + p.w + slack
                && c.y + c.h <= p.y + p.h + slack,
            "child {c:?} escapes parent {p:?}"
        );
        assert_nested(map, child);
    }
}

fn assert_no_overlap(tiles: &[Tile]) {
    for (i, a) in tiles.iter().enumerate() {
        for b in &tiles[i + 1..] {
            // Touching edges are fine; genuine overlap is not.
            let overlap_w = (a.rect.x + a.rect.w).min(b.rect.x + b.rect.w) - a.rect.x.max(b.rect.x);
            let overlap_h = (a.rect.y + a.rect.h).min(b.rect.y + b.rect.h) - a.rect.y.max(b.rect.y);
            assert!(
                overlap_w <= 1e-6 || overlap_h <= 1e-6,
                "tiles overlap: {:?} and {:?}",
                a.rect,
                b.rect
            );
        }
    }
}

#[test]
fn the_root_tile_fills_the_canvas() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    let root = map.root().unwrap();
    assert_eq!(root.node, tree.root());
    assert_eq!(root.rect, full_canvas());
    assert_eq!(root.depth, 0);
}

#[test]
fn siblings_fill_their_parent_without_overlapping() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    let root = map.root().unwrap();
    let children: Vec<Tile> = map.children_of(root).to_vec();
    assert!(children.len() >= 3);

    let covered: f64 = children.iter().map(|t| t.rect.area()).sum();
    // Zero-sized entries are dropped, so the children cover all of the area
    // that carries any bytes.
    assert!(
        (covered - root.rect.area()).abs() / root.rect.area() < 0.01,
        "children cover {covered} of {}",
        root.rect.area()
    );
    assert_no_overlap(&children);
}

#[test]
fn area_is_proportional_to_size() {
    let (_d, tree) = fixture();
    let canvas = full_canvas();
    let map = layout(&tree, tree.root(), canvas, &no_padding());
    let root = map.root().unwrap();

    let total = tree.node(tree.root()).size as f64;
    for tile in map.children_of(root) {
        let expected = canvas.area() * (tree.node(tile.node).size as f64 / total);
        let ratio = tile.rect.area() / expected;
        assert!(
            (0.97..1.03).contains(&ratio),
            "{} has area ratio {ratio}",
            tree.node(tile.node).name
        );
    }
}

#[test]
fn the_biggest_entry_gets_the_biggest_tile() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    let root = map.root().unwrap();
    let largest = map
        .children_of(root)
        .iter()
        .max_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
        .unwrap();
    assert_eq!(tree.node(largest.node).name, "big");
}

#[test]
fn nesting_is_geometric_at_every_level() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &LayoutOptions::default());
    assert_nested(&map, map.root().unwrap());
}

#[test]
fn aspect_ratios_stay_reasonable() {
    // The whole point of squarifying: tiles should be close to square rather
    // than thin slivers.
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    let root = map.root().unwrap();

    for tile in map.children_of(root) {
        let (w, h) = (tile.rect.w, tile.rect.h);
        assert!(w > 0.0 && h > 0.0, "degenerate tile {:?}", tile.rect);
        let ratio = (w / h).max(h / w);
        assert!(ratio < 12.0, "sliver: {:?} has ratio {ratio}", tile.rect);
    }
}

// ------------------------------------------------------------- level of detail

#[test]
fn a_large_min_area_stops_subdivision_early() {
    let (_d, tree) = fixture();
    let detailed = layout(&tree, tree.root(), full_canvas(), &no_padding());
    let coarse = layout(
        &tree,
        tree.root(),
        full_canvas(),
        &LayoutOptions {
            min_area: 50_000.0,
            padding: 0.0,
            max_depth: None,
        },
    );
    assert!(
        coarse.len() < detailed.len(),
        "coarse {} should hold fewer tiles than detailed {}",
        coarse.len(),
        detailed.len()
    );
}

#[test]
fn a_tiny_canvas_produces_almost_nothing() {
    let (_d, tree) = fixture();
    // Smaller than min_area: only the root survives.
    let map = layout(
        &tree,
        tree.root(),
        Rect::new(0.0, 0.0, 2.0, 2.0),
        &LayoutOptions::default(),
    );
    assert_eq!(map.len(), 1);
    assert!(map.root().unwrap().truncated, "there is more inside");
}

#[test]
fn max_depth_is_respected() {
    let (_d, tree) = fixture();
    let map = layout(
        &tree,
        tree.root(),
        full_canvas(),
        &LayoutOptions {
            min_area: 1.0,
            padding: 0.0,
            max_depth: Some(1),
        },
    );
    assert!(map.tiles().iter().all(|t| t.depth <= 1));
    assert!(map.tiles().iter().any(|t| t.depth == 1));
}

#[test]
fn a_leaf_is_never_marked_truncated() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    for tile in map.tiles() {
        if tree.node(tile.node).children_len == 0 {
            assert!(!tile.truncated, "a file cannot hide anything");
        }
    }
}

#[test]
fn a_zero_area_canvas_yields_an_empty_map() {
    let (_d, tree) = fixture();
    let map = layout(
        &tree,
        tree.root(),
        Rect::new(0.0, 0.0, 0.0, 500.0),
        &LayoutOptions::default(),
    );
    assert!(map.is_empty());
    assert!(map.root().is_none());
    assert!(map.hit(0.0, 0.0).is_none());
    assert!(map.visible(full_canvas()).is_empty());
}

// ------------------------------------------------------------------ hit-testing

#[test]
fn hit_testing_returns_the_deepest_tile_under_the_point() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    // Pick a leaf and aim at the middle of it.
    let leaf = map
        .tiles()
        .iter()
        .filter(|t| !t.has_children() && t.rect.area() > 100.0)
        .max_by(|a, b| a.depth.cmp(&b.depth))
        .expect("a nested leaf");
    let (cx, cy) = (
        leaf.rect.x + leaf.rect.w / 2.0,
        leaf.rect.y + leaf.rect.h / 2.0,
    );

    let hit = map.hit(cx, cy).expect("the point is inside the map");
    assert_eq!(hit.node, leaf.node);
}

#[test]
fn every_tile_can_be_hit_at_its_own_centre() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    for tile in map.tiles().iter().filter(|t| !t.has_children()) {
        if tile.rect.w < 2.0 || tile.rect.h < 2.0 {
            continue; // too thin to aim at reliably
        }
        let (cx, cy) = (
            tile.rect.x + tile.rect.w / 2.0,
            tile.rect.y + tile.rect.h / 2.0,
        );
        let hit = map
            .hit(cx, cy)
            .expect("centre of a tile must hit something");
        assert_eq!(
            hit.node, tile.node,
            "centre of {:?} hit a different node",
            tile.rect
        );
    }
}

#[test]
fn a_point_outside_the_map_hits_nothing() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    assert!(map.hit(-1.0, 10.0).is_none());
    assert!(map.hit(10.0, -1.0).is_none());
    assert!(
        map.hit(1000.0, 10.0).is_none(),
        "the right edge is exclusive"
    );
    assert!(map.hit(5000.0, 5000.0).is_none());
}

#[test]
fn the_path_to_a_point_is_a_chain_of_nested_tiles() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    let leaf = map
        .tiles()
        .iter()
        .filter(|t| !t.has_children() && t.rect.area() > 100.0)
        .max_by(|a, b| a.depth.cmp(&b.depth))
        .unwrap();
    let path = map.path_to(
        leaf.rect.x + leaf.rect.w / 2.0,
        leaf.rect.y + leaf.rect.h / 2.0,
    );

    assert_eq!(path.first().unwrap().node, tree.root());
    assert_eq!(path.last().unwrap().node, leaf.node);
    for pair in path.windows(2) {
        assert_eq!(pair[1].depth, pair[0].depth + 1);
    }
}

// ---------------------------------------------------------------- culling

#[test]
fn culling_returns_only_intersecting_tiles() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
    let visible = map.visible(viewport);

    assert!(!visible.is_empty());
    for tile in &visible {
        assert!(
            tile.rect.intersects(&viewport),
            "{:?} does not touch the viewport",
            tile.rect
        );
    }
}

#[test]
fn culling_a_viewport_covering_everything_returns_everything() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    assert_eq!(map.visible(full_canvas()).len(), map.len());
}

#[test]
fn a_viewport_off_the_map_returns_nothing() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());
    assert!(map
        .visible(Rect::new(5000.0, 5000.0, 10.0, 10.0))
        .is_empty());
}

#[test]
fn culling_never_misses_a_tile_that_a_full_scan_would_find() {
    // The hierarchy skip is only sound if a parent always covers its children;
    // compare it against the naive filter to prove it.
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    for viewport in [
        Rect::new(0.0, 0.0, 250.0, 250.0),
        Rect::new(400.0, 200.0, 300.0, 300.0),
        Rect::new(900.0, 600.0, 200.0, 200.0),
        Rect::new(-50.0, -50.0, 120.0, 120.0),
    ] {
        // Compare as sets: culling walks the hierarchy depth-first while the
        // naive filter runs in arena order, so the orders legitimately differ.
        let mut culled: Vec<NodeId> = map.visible(viewport).iter().map(|t| t.node).collect();
        let mut naive: Vec<NodeId> = map
            .tiles()
            .iter()
            .filter(|t| t.rect.intersects(&viewport))
            .map(|t| t.node)
            .collect();
        culled.sort_unstable();
        naive.sort_unstable();
        assert_eq!(culled, naive, "culling disagreed for {viewport:?}");
    }
}

/// What the order actually has to guarantee: a parent is emitted before its own
/// children, so painting them in sequence draws children on top.
#[test]
fn culling_emits_parents_before_their_children() {
    let (_d, tree) = fixture();
    let map = layout(&tree, tree.root(), full_canvas(), &no_padding());

    for viewport in [
        full_canvas(),
        Rect::new(0.0, 0.0, 250.0, 250.0),
        Rect::new(300.0, 100.0, 400.0, 400.0),
    ] {
        let visible = map.visible(viewport);
        let position: std::collections::HashMap<NodeId, usize> = visible
            .iter()
            .enumerate()
            .map(|(i, t)| (t.node, i))
            .collect();

        for tile in &visible {
            for child in map.children_of(tile) {
                let Some(child_at) = position.get(&child.node) else {
                    continue; // culled away, which is fine
                };
                assert!(
                    position[&tile.node] < *child_at,
                    "child {} drawn before its parent {}",
                    tree.node(child.node).name,
                    tree.node(tile.node).name
                );
            }
        }
    }
}

// ------------------------------------------------------------------ scale

#[test]
fn a_wide_tree_lays_out_without_slivers_or_panics() {
    let dir = tempfile::tempdir().unwrap();
    // Sizes spanning three orders of magnitude, the shape that breaks naive
    // slice-and-dice layouts.
    for i in 0..400 {
        fs::write(
            dir.path().join(format!("f{i:04}.bin")),
            vec![0u8; 1000 + i * 500],
        )
        .unwrap();
    }
    let (tree, _) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    let canvas = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let map = layout(&tree, tree.root(), canvas, &no_padding());
    let root = map.root().unwrap();

    assert_eq!(map.children_of(root).len(), 400);
    assert_no_overlap(map.children_of(root));

    let covered: f64 = map.children_of(root).iter().map(|t| t.rect.area()).sum();
    assert!((covered - canvas.area()).abs() / canvas.area() < 0.01);
}
