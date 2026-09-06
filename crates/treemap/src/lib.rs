//! Squarified treemap layout.
//!
//! Turns a scanned tree into rectangles that can be drawn. The layout itself is
//! Bruls, Huizing and van Wijk's squarified algorithm (2000): lay children out
//! in rows, extending a row while doing so improves the worst aspect ratio in
//! it, and closing the row when it stops.
//!
//! Two things beyond the plain algorithm matter for a real disk:
//!
//! **Level of detail.** A 2 TB disk has millions of entries and a screen has a
//! couple of million pixels, so most of the tree can never be seen. Subdivision
//! stops once a rectangle is smaller than [`LayoutOptions::min_area`], which
//! keeps the number of tiles proportional to the screen rather than the disk.
//!
//! **Hierarchy instead of a spatial index.** Tiles are emitted as an arena with
//! children contiguous after their parent — the same layout `scan-core` uses.
//! Because a child's rectangle is always inside its parent's, hit-testing walks
//! down from the root and culling skips whole subtrees whose parent is off
//! screen. That is what a quadtree would have been for, except the structure is
//! already there and costs nothing to keep.

use spacetrace_scan_core::{NodeId, Tree};

/// An axis-aligned rectangle in layout space, y growing downward.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn area(&self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    /// Shrink by `pad` on every side, never past zero.
    fn deflate(&self, pad: f64) -> Rect {
        let w = (self.w - 2.0 * pad).max(0.0);
        let h = (self.h - 2.0 * pad).max(0.0);
        Rect {
            x: self.x + pad,
            y: self.y + pad,
            w,
            h,
        }
    }
}

/// One laid-out rectangle.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Tile {
    /// The entry this rectangle stands for, as an index into the scanned tree.
    pub node: NodeId,
    pub rect: Rect,
    /// Levels below the layout root; the root tile is 0.
    pub depth: u16,
    /// True when subdivision stopped here because of the area limit even though
    /// the entry has children. The renderer can hint that there is more inside.
    pub truncated: bool,
    children_start: u32,
    children_len: u32,
}

impl Tile {
    pub fn has_children(&self) -> bool {
        self.children_len > 0
    }
}

#[derive(Debug, Clone)]
pub struct LayoutOptions {
    /// Stop subdividing a rectangle smaller than this, in square layout units.
    ///
    /// Around 4–6 px² is the useful range: below that a tile cannot carry a
    /// border, let alone a label, so splitting it further only costs work.
    pub min_area: f64,
    /// Inset applied to a directory before its children are laid out, so nesting
    /// is visible. Skipped when the rectangle is too small to afford it.
    pub padding: f64,
    /// Never go deeper than this many levels below the layout root.
    pub max_depth: Option<u16>,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            min_area: 6.0,
            padding: 1.0,
            max_depth: None,
        }
    }
}

/// A laid-out treemap: tiles in an arena, parents before children.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TileTree {
    tiles: Vec<Tile>,
}

impl TileTree {
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn root(&self) -> Option<&Tile> {
        self.tiles.first()
    }

    pub fn children_of(&self, tile: &Tile) -> &[Tile] {
        let start = tile.children_start as usize;
        &self.tiles[start..start + tile.children_len as usize]
    }

    /// The deepest tile containing the point, or `None` when the point is
    /// outside the map.
    ///
    /// Descends the hierarchy instead of searching, so the cost is the depth of
    /// the tree rather than the number of tiles.
    pub fn hit(&self, x: f64, y: f64) -> Option<&Tile> {
        let mut current = self.tiles.first()?;
        if !current.rect.contains(x, y) {
            return None;
        }
        loop {
            // Padding means a point can be inside a parent but in none of its
            // children; the parent is then the right answer.
            let Some(next) = self
                .children_of(current)
                .iter()
                .find(|c| c.rect.contains(x, y))
            else {
                return Some(current);
            };
            current = next;
        }
    }

    /// Every tile intersecting `viewport`, parents before children.
    ///
    /// A subtree whose parent misses the viewport is skipped whole, which is
    /// what keeps panning cheap on a large map.
    pub fn visible(&self, viewport: Rect) -> Vec<&Tile> {
        let mut out = Vec::new();
        if let Some(root) = self.tiles.first() {
            self.collect_visible(root, &viewport, &mut out);
        }
        out
    }

    fn collect_visible<'a>(&'a self, tile: &'a Tile, viewport: &Rect, out: &mut Vec<&'a Tile>) {
        if !tile.rect.intersects(viewport) {
            return;
        }
        out.push(tile);
        for child in self.children_of(tile) {
            self.collect_visible(child, viewport, out);
        }
    }

    /// Path from the root tile down to the tile at the given point, useful for
    /// breadcrumbs.
    pub fn path_to(&self, x: f64, y: f64) -> Vec<&Tile> {
        let mut path = Vec::new();
        let Some(mut current) = self.tiles.first() else {
            return path;
        };
        if !current.rect.contains(x, y) {
            return path;
        }
        path.push(current);
        while let Some(next) = self
            .children_of(current)
            .iter()
            .find(|c| c.rect.contains(x, y))
        {
            path.push(next);
            current = next;
        }
        path
    }
}

/// Lay out the subtree rooted at `root` inside `bounds`.
pub fn layout(tree: &Tree, root: NodeId, bounds: Rect, opts: &LayoutOptions) -> TileTree {
    let mut tiles = Vec::new();
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return TileTree { tiles };
    }

    tiles.push(Tile {
        node: root,
        rect: bounds,
        depth: 0,
        truncated: tree.node(root).children_len > 0,
        children_start: 0,
        children_len: 0,
    });
    // Breadth-first so that children of a tile land in one contiguous run,
    // which is what makes `children_of` a slice rather than a search.
    let mut queue = std::collections::VecDeque::from([0usize]);

    while let Some(index) = queue.pop_front() {
        let (node, rect, depth) = {
            let t = &tiles[index];
            (t.node, t.rect, t.depth)
        };

        if opts.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let inner = if rect.w > 2.0 * opts.padding && rect.h > 2.0 * opts.padding {
            rect.deflate(opts.padding)
        } else {
            rect
        };
        if inner.area() < opts.min_area {
            continue;
        }

        let children = sorted_children(tree, node);
        if children.is_empty() {
            tiles[index].truncated = false;
            continue;
        }

        let placed = squarify(&children, inner, opts.min_area);
        if placed.is_empty() {
            continue;
        }

        let start = tiles.len();
        for (child, child_rect) in placed {
            tiles.push(Tile {
                node: child,
                rect: child_rect,
                depth: depth + 1,
                truncated: tree.node(child).children_len > 0,
                children_start: 0,
                children_len: 0,
            });
        }
        tiles[index].children_start = start as u32;
        tiles[index].children_len = (tiles.len() - start) as u32;
        // Everything that fitted got a tile, so nothing was hidden here.
        tiles[index].truncated = false;

        for child_index in start..tiles.len() {
            queue.push_back(child_index);
        }
    }

    TileTree { tiles }
}

/// Children with a non-zero size, largest first. Zero-sized entries are dropped:
/// they would take no area, and keeping them only produces degenerate rectangles.
fn sorted_children(tree: &Tree, node: NodeId) -> Vec<(NodeId, f64)> {
    let mut children: Vec<(NodeId, f64)> = tree
        .children(node)
        .map(|c| (c, tree.node(c).size as f64))
        .filter(|(_, size)| *size > 0.0)
        .collect();
    children.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    children
}

/// The squarified pass: fill `rect` with `items`, proportionally by value.
fn squarify(items: &[(NodeId, f64)], rect: Rect, min_area: f64) -> Vec<(NodeId, Rect)> {
    let total: f64 = items.iter().map(|(_, v)| v).sum();
    if total <= 0.0 || rect.area() <= 0.0 {
        return Vec::new();
    }

    // Work in area units so a row's values can be compared with the free space
    // directly rather than being rescaled at every step.
    let scale = rect.area() / total;
    let remaining: Vec<(NodeId, f64)> = items.iter().map(|(n, v)| (*n, v * scale)).collect();

    let mut out = Vec::with_capacity(remaining.len());
    let mut free = rect;
    let mut cursor = 0usize;

    while cursor < remaining.len() {
        let side = free.w.min(free.h);
        if side <= 0.0 {
            break;
        }

        // Extend the row while the worst aspect ratio in it keeps improving.
        let mut row_end = cursor + 1;
        let mut row_sum = remaining[cursor].1;
        let mut best = worst_ratio(row_sum, remaining[cursor].1, remaining[cursor].1, side);

        while row_end < remaining.len() {
            let next = remaining[row_end].1;
            let sum = row_sum + next;
            // The list is sorted descending, so the first item is the largest
            // and the newcomer is the smallest.
            let candidate = worst_ratio(sum, remaining[cursor].1, next, side);
            if candidate > best {
                break;
            }
            best = candidate;
            row_sum = sum;
            row_end += 1;
        }

        free = place_row(&remaining[cursor..row_end], row_sum, free, &mut out);
        cursor = row_end;

        // Everything left would be slivers; stop rather than emit them.
        if free.area() < min_area {
            break;
        }
    }

    out
}

/// Worst aspect ratio of a row holding `sum` area across `side`, given its
/// largest and smallest members.
fn worst_ratio(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || min <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = sum * sum;
    let w2 = side * side;
    ((w2 * max) / s2).max(s2 / (w2 * min))
}

/// Place one row along the shorter side of `free` and return what is left.
fn place_row(
    row: &[(NodeId, f64)],
    row_sum: f64,
    free: Rect,
    out: &mut Vec<(NodeId, Rect)>,
) -> Rect {
    if row_sum <= 0.0 {
        return free;
    }
    let horizontal = free.w >= free.h;
    // Thickness of the band the row occupies, across the shorter side.
    let thickness = if horizontal {
        (row_sum / free.h).min(free.w)
    } else {
        (row_sum / free.w).min(free.h)
    };

    let mut offset = 0.0;
    for (index, (node, value)) in row.iter().enumerate() {
        let share = value / row_sum;
        if horizontal {
            let mut h = free.h * share;
            // Absorb rounding into the last tile so the band is exactly filled.
            if index + 1 == row.len() {
                h = (free.y + free.h) - (free.y + offset);
            }
            out.push((*node, Rect::new(free.x, free.y + offset, thickness, h)));
            offset += h;
        } else {
            let mut w = free.w * share;
            if index + 1 == row.len() {
                w = (free.x + free.w) - (free.x + offset);
            }
            out.push((*node, Rect::new(free.x + offset, free.y, w, thickness)));
            offset += w;
        }
    }

    if horizontal {
        Rect::new(
            free.x + thickness,
            free.y,
            (free.w - thickness).max(0.0),
            free.h,
        )
    } else {
        Rect::new(
            free.x,
            free.y + thickness,
            free.w,
            (free.h - thickness).max(0.0),
        )
    }
}
