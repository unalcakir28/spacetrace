import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./treemap.css";
import { squarify, type Rect } from "./squarify";
import {
  TREE,
  dominant,
  entriesOf,
  measure,
  nodeAt,
  type Basis,
  type Category,
  type Node,
} from "./tree";
import { bytes, count, percent } from "./format";

export interface TreemapLabels {
  zoomHint: string;
  tapHint: string;
  backHint: string;
  rootLabel: string;
  basisLabel: string;
  onDisk: string;
  logical: string;
  basisHelp: string;
  sparseCallout: string;
  selected: string;
  share: string;
  kind: string;
  sparse: string;
  empty: string;
  emptyHelp: string;
  entries: string;
  categories: Record<Category, string>;
}

interface Props {
  labels: TreemapLabels;
  locale: string;
}

/** Category colours come from the stylesheet, so the map and the rest of the
 *  page can never disagree about what a colour means. */
function colorOf(category: Category): string {
  return `var(--cat-${category})`;
}

/** `other` and `directory` are surface tones rather than hues, so dark label
 *  text on them is the one place the palette does not carry its own contrast. */
const DARK_TILES: Category[] = ["other", "directory"];

const GAP = 3;

export default function Treemap({ labels, locale }: Props) {
  const [path, setPath] = useState<string[]>([]);
  const [basis, setBasis] = useState<Basis>("alloc");
  const [selectedName, setSelectedName] = useState<string | null>(null);
  // A plausible desktop rectangle rather than zero, so the server renders a
  // complete map and the tiles are there before — and without — hydration. The
  // ResizeObserver corrects it on mount; because the tiles are absolutely
  // positioned inside a fixed-height box, that correction shifts nothing else
  // on the page.
  const [box, setBox] = useState({ w: 880, h: 380 });
  const surface = useRef<HTMLDivElement>(null);

  // The rectangle has to be measured, not assumed: the layout depends on the
  // aspect ratio, and on a phone it is nothing like the desktop one.
  useEffect(() => {
    const el = surface.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setBox({ w: width, h: height });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const current = useMemo(() => nodeAt(path), [path]);
  const children = current.children ?? [];
  const total = useMemo(() => measure(current, basis), [current, basis]);

  const placed = useMemo(() => {
    if (box.w <= 0 || box.h <= 0) return [];
    const bounds: Rect = { x: 0, y: 0, w: box.w, h: box.h };
    return squarify(
      children.map((child) => ({ item: child, value: measure(child, basis) })),
      bounds,
    );
  }, [children, basis, box.w, box.h]);

  const selected = useMemo(
    () => children.find((child) => child.name === selectedName) ?? null,
    [children, selectedName],
  );

  const enter = useCallback((node: Node) => {
    if (!node.children?.length) return;
    setPath((previous) => [...previous, node.name]);
    setSelectedName(null);
  }, []);

  const up = useCallback(() => {
    setPath((previous) => previous.slice(0, -1));
    setSelectedName(null);
  }, []);

  // Backspace is what the application uses, so the demo uses it too — but only
  // while the map itself has focus, or it would hijack the browser's own
  // shortcut for the rest of the page.
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== "Backspace" || path.length === 0) return;
    event.preventDefault();
    up();
  };

  const crumbs = [labels.rootLabel, ...path];

  return (
    <div className="tm" onKeyDown={onKeyDown}>
      <div className="tm-bar">
        <nav className="tm-crumbs" aria-label={labels.rootLabel}>
          {crumbs.map((name, index) => {
            const last = index === crumbs.length - 1;
            return (
              <span key={`${name}-${index}`} className="tm-crumb">
                {index > 0 && (
                  <span className="tm-sep" aria-hidden="true">
                    /
                  </span>
                )}
                {last ? (
                  <span aria-current="location">{name}</span>
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setPath(path.slice(0, index));
                      setSelectedName(null);
                    }}
                  >
                    {name}
                  </button>
                )}
              </span>
            );
          })}
        </nav>

        <p className="tm-hint" aria-hidden="true">
          <span className="on-pointer">{labels.zoomHint}</span>
          <span className="on-touch">{labels.tapHint}</span>
        </p>

        <div className="tm-controls">
          <button
            type="button"
            className="tm-up"
            onClick={up}
            disabled={path.length === 0}
          >
            <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
              <path
                d="M8 13V4m0 0L4.2 7.8M8 4l3.8 3.8"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
              />
            </svg>
            {labels.backHint}
          </button>

          <div
            className="tm-basis"
            role="group"
            aria-label={labels.basisLabel}
          >
            <button
              type="button"
              aria-pressed={basis === "alloc"}
              onClick={() => setBasis("alloc")}
            >
              {labels.onDisk}
            </button>
            <button
              type="button"
              aria-pressed={basis === "size"}
              onClick={() => setBasis("size")}
            >
              {labels.logical}
            </button>
          </div>
        </div>
      </div>

      <div className="tm-body">
        <div className="tm-surface" ref={surface}>
          {/* Keyed by path and measure so the tiles remount and the staggered
              arrival replays: the map materialises rather than snapping. */}
          <div className="tm-tiles" key={`${path.join("/")}::${basis}`}>
            {placed.map(({ item, rect }, index) => {
              const value = measure(item, basis);
              const category = dominant(item, basis);
              const isFolder = Boolean(item.children?.length);
              const isSelected = item.name === selectedName;
              const wide = rect.w > 78 && rect.h > 30;
              const roomForSize = rect.w > 96 && rect.h > 46;
              return (
                <button
                  type="button"
                  key={item.name}
                  className={`tm-tile${isSelected ? " sel" : ""}${
                    DARK_TILES.includes(category) ? " on-dark" : ""
                  }`}
                  style={{
                    left: rect.x + GAP / 2,
                    top: rect.y + GAP / 2,
                    width: Math.max(0, rect.w - GAP),
                    height: Math.max(0, rect.h - GAP),
                    background: colorOf(category),
                    animationDelay: `${Math.min(index * 30, 260)}ms`,
                  }}
                  onClick={() => setSelectedName(item.name)}
                  onDoubleClick={() => enter(item)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" || !isFolder) return;
                    event.preventDefault();
                    enter(item);
                  }}
                  aria-label={`${item.name}, ${bytes(value, locale)}${
                    isFolder ? ", " + labels.zoomHint : ""
                  }`}
                  title={`${item.name} — ${bytes(value, locale)}`}
                >
                  {wide && (
                    <span className="tm-name">
                      {item.name}
                      {isFolder && <span aria-hidden="true">/</span>}
                    </span>
                  )}
                  {roomForSize && (
                    <span className="tm-size">{bytes(value, locale)}</span>
                  )}
                  {/* The `~` on its own says "these two numbers disagree";
                      printing the other measure beside it says by how much,
                      which is the part worth knowing. */}
                  {roomForSize && item.sparse && (
                    <span className="tm-flag" aria-hidden="true">
                      ~{" "}
                      {bytes(
                        measure(item, basis === "alloc" ? "size" : "alloc"),
                        locale,
                      )}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </div>

        <aside className="tm-inspect" aria-live="polite">
          {selected ? (
            <>
              <h3>{selected.name}</h3>
              <dl>
                <dt>{labels.onDisk}</dt>
                <dd>{bytes(measure(selected, "alloc"), locale)}</dd>
                <dt>{labels.logical}</dt>
                <dd>{bytes(measure(selected, "size"), locale)}</dd>
                <dt>{labels.share}</dt>
                <dd>
                  {total > 0
                    ? percent(measure(selected, basis) / total, locale)
                    : "—"}
                </dd>
                <dt>{labels.kind}</dt>
                <dd>
                  {labels.categories[dominant(selected, basis)]}
                  {selected.sparse ? ` · ${labels.sparse}` : ""}
                </dd>
                <dt>{labels.entries}</dt>
                <dd>{count(entriesOf(selected), locale)}</dd>
              </dl>
              {selected.sparse && (
                <p className="tm-sparse">{labels.sparseCallout}</p>
              )}
            </>
          ) : (
            <>
              <h3 className="tm-quiet">{labels.empty}</h3>
              <p className="tm-quiet-help">{labels.emptyHelp}</p>
              {basis === "size" ? (
                <p className="tm-sparse tm-spaced">{labels.sparseCallout}</p>
              ) : (
                <p className="tm-basis-help">{labels.basisHelp}</p>
              )}
            </>
          )}
        </aside>
      </div>
    </div>
  );
}
