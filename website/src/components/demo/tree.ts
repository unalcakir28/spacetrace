/**
 * The demo tree. Every figure is plausible for a real developer's home folder
 * and, more importantly, the relationship between the two measures is honest:
 *
 *  - `Docker.raw` is sparse. It claims a terabyte and occupies nineteen
 *    gigabytes, which is why flipping the measure destroys the map — the whole
 *    reason the app defaults to on disk.
 *  - `node_modules` is the opposite case: tens of thousands of tiny files, each
 *    rounded up to a block, so it takes *more* on disk than it claims.
 *
 * Both divergences run in the direction they really do. A demo that made the
 * two measures agree would be advertising a product that does not exist.
 */

export type Category =
  | "directory"
  | "image"
  | "video"
  | "audio"
  | "document"
  | "archive"
  | "code"
  | "binary"
  | "cache"
  | "other";

export interface Node {
  name: string;
  category: Category;
  /** Logical bytes — the length the file claims. Leaves only. */
  size?: number;
  /** On-disk bytes — blocks actually allocated. Leaves only. */
  alloc?: number;
  sparse?: boolean;
  entries?: number;
  children?: Node[];
}

const G = 1024 ** 3;
const M = 1024 ** 2;
const T = 1024 ** 4;

export const TREE: Node = {
  name: "you",
  category: "directory",
  children: [
    {
      name: "Library",
      category: "directory",
      children: [
        {
          name: "Containers",
          category: "directory",
          children: [
            {
              name: "Docker.raw",
              category: "archive",
              alloc: 19.4 * G,
              size: 1.0 * T,
              sparse: true,
              entries: 1,
            },
            {
              name: "Group Containers",
              category: "other",
              alloc: 2.1 * G,
              size: 2.05 * G,
              entries: 9412,
            },
            {
              name: "com.apple.Safari",
              category: "cache",
              alloc: 1.2 * G,
              size: 1.18 * G,
              entries: 3180,
            },
          ],
        },
        {
          name: "Caches",
          category: "cache",
          alloc: 8.4 * G,
          size: 8.26 * G,
          entries: 84120,
        },
        {
          name: "Application Support",
          category: "other",
          alloc: 6.2 * G,
          size: 6.1 * G,
          entries: 41208,
        },
        {
          name: "Developer",
          category: "binary",
          alloc: 3.1 * G,
          size: 3.05 * G,
          entries: 2140,
        },
      ],
    },
    {
      name: "Pictures",
      category: "directory",
      children: [
        {
          name: "Photos Library.photoslibrary",
          category: "image",
          alloc: 11.2 * G,
          size: 11.19 * G,
          entries: 24310,
        },
        {
          name: "Screenshots",
          category: "image",
          alloc: 1.5 * G,
          size: 1.44 * G,
          entries: 2871,
        },
      ],
    },
    {
      name: "projects",
      category: "directory",
      children: [
        {
          // More on disk than it claims: 214k files, every one rounded up to a
          // whole block.
          name: "node_modules",
          category: "code",
          alloc: 4.1 * G,
          size: 3.62 * G,
          entries: 214880,
        },
        {
          name: "target",
          category: "binary",
          alloc: 2.0 * G,
          size: 1.98 * G,
          entries: 18402,
        },
        {
          name: "src",
          category: "code",
          alloc: 148 * M,
          size: 121 * M,
          entries: 3106,
        },
      ],
    },
    {
      name: "Movies",
      category: "directory",
      children: [
        {
          name: "screencast-2026-09.mov",
          category: "video",
          alloc: 3.4 * G,
          size: 3.4 * G,
          entries: 1,
        },
        {
          name: "raw",
          category: "video",
          alloc: 1.5 * G,
          size: 1.5 * G,
          entries: 24,
        },
      ],
    },
    {
      name: "Music",
      category: "audio",
      alloc: 2.2 * G,
      size: 2.19 * G,
      entries: 6420,
    },
    {
      name: "Downloads",
      category: "document",
      alloc: 1.8 * G,
      size: 1.78 * G,
      entries: 412,
    },
    {
      name: "Documents",
      category: "document",
      alloc: 912 * M,
      size: 906 * M,
      entries: 1840,
    },
  ],
};

export type Basis = "alloc" | "size";

/** A folder's figure is the sum of its children; a leaf carries its own. */
export function measure(node: Node, basis: Basis): number {
  if (node.children?.length) {
    return node.children.reduce((sum, child) => sum + measure(child, basis), 0);
  }
  return (basis === "alloc" ? node.alloc : node.size) ?? 0;
}

export function entriesOf(node: Node): number {
  if (node.children?.length) {
    return node.children.reduce((sum, child) => sum + entriesOf(child), 0);
  }
  return node.entries ?? 1;
}

/**
 * A folder has no file type of its own, so it takes the colour of what it is
 * *full of* — the largest child followed down to a file. Colouring folders by
 * "directory" paints every one of them the same, which is no information.
 */
export function dominant(node: Node, basis: Basis): Category {
  if (!node.children?.length) return node.category;
  let best: Node | null = null;
  let bestValue = -1;
  for (const child of node.children) {
    const value = measure(child, basis);
    if (value > bestValue) {
      bestValue = value;
      best = child;
    }
  }
  return best ? dominant(best, basis) : node.category;
}

/** Walk a path of names from the root; unknown names stop the walk. */
export function nodeAt(path: string[]): Node {
  let node = TREE;
  for (const name of path) {
    const next = node.children?.find((child) => child.name === name);
    if (!next) break;
    node = next;
  }
  return node;
}
