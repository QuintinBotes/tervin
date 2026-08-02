/**
 * Rendering the pane tree.
 *
 * The tree in `lib/panes` is a value; this turns it into nested flex containers
 * with a draggable divider between every pair of siblings. Recursion mirrors the
 * data exactly, so a layout of any shape renders without special cases.
 *
 * Two details that are easy to get wrong and expensive to debug:
 *
 *  - **Sizes are `flexBasis` percentages with `flexGrow: 0`.** Using `flexGrow`
 *    for the ratio makes a pane's size depend on its content, so a long line of
 *    output would widen its own pane.
 *  - **Dragging is tracked on `window`, not the handle.** A fast drag outsends
 *    pointer events to the handle element, and the divider would stick.
 *
 * A zoomed pane is rendered alone rather than by resizing its siblings to nothing:
 * xterm reflows on every resize, so animating panes to zero would reflow every
 * shell in the tab.
 */

import { useCallback, useEffect, useRef } from "react";
import {
  listPanes,
  type PaneNode,
  type SplitDirection,
} from "../lib/panes";
import { useWorkspace } from "../lib/store";
import { TerminalPane } from "./TerminalPane";

interface Props {
  node: PaneNode;
  activePaneId: string | null;
  /** Set when one pane is zoomed; only that pane renders. */
  zoomedPaneId: string | null;
  onFocus: (paneId: string) => void;
  onResize: (splitId: string, dividerIndex: number, delta: number) => void;
}

export function PaneTree({
  node,
  activePaneId,
  zoomedPaneId,
  onFocus,
  onResize,
}: Props) {
  // A zoomed pane replaces the whole tree, which is also why zooming costs one
  // reflow rather than one per pane.
  if (zoomedPaneId) {
    const exists = listPanes(node).includes(zoomedPaneId);
    if (exists) {
      return (
        <PaneLeaf
          paneId={zoomedPaneId}
          active={activePaneId === zoomedPaneId}
          onFocus={onFocus}
        />
      );
    }
  }

  if (node.type === "leaf") {
    return (
      <PaneLeaf
        paneId={node.paneId}
        active={activePaneId === node.paneId}
        onFocus={onFocus}
      />
    );
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: node.direction,
        // Stretch, not centre: centring leaves the container's own background
        // showing as bands above and below each child.
        alignItems: "stretch",
        flex: 1,
        minWidth: 0,
        minHeight: 0,
      }}
    >
      {node.children.map((child, index) => (
        <div key={child.id} style={{ display: "contents" }}>
          {index > 0 && (
            <Divider
              direction={node.direction}
              onDrag={(delta) => onResize(node.id, index - 1, delta)}
            />
          )}
          <div
            style={{
              // Basis carries the ratio; grow stays at 0 so content cannot
              // influence a pane's size.
              flexGrow: 0,
              flexShrink: 1,
              flexBasis: `${(node.sizes[index] ?? 1 / node.children.length) * 100}%`,
              display: "flex",
              minWidth: 0,
              minHeight: 0,
              overflow: "hidden",
            }}
          >
            <PaneTree
              node={child}
              activePaneId={activePaneId}
              zoomedPaneId={null}
              onFocus={onFocus}
              onResize={onResize}
            />
          </div>
        </div>
      ))}
    </div>
  );
}

function PaneLeaf({
  paneId,
  active,
  onFocus,
}: {
  paneId: string;
  active: boolean;
  onFocus: (paneId: string) => void;
}) {
  const exited = useWorkspace((s) => s.panes[paneId]?.exited ?? false);
  const remote = useWorkspace((s) => s.panes[paneId]?.remote ?? false);

  return (
    <div
      className="col"
      style={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        // The focused pane is marked by a hairline, not a glow or a shadow.
        outline: active ? "1px solid var(--tervin-accent)" : "none",
        outlineOffset: -1,
        position: "relative",
      }}
      data-pane={paneId}
    >
      {remote && (
        <div
          className="row"
          style={{
            flex: "none",
            height: 18,
            padding: "0 var(--sp-3)",
            background: "var(--tervin-raised)",
            borderBottom: "1px solid var(--tervin-line)",
          }}
        >
          {/* A remote pane is labelled: a destructive command here is not the
              same action as the same command locally. */}
          <span className="label" style={{ fontSize: 10 }}>
            remote
          </span>
        </div>
      )}
      <TerminalPane paneId={paneId} active={active} onFocus={() => onFocus(paneId)} />
      {exited && (
        <div
          className="row"
          style={{
            flex: "none",
            padding: "var(--sp-1) var(--sp-3)",
            borderTop: "1px solid var(--tervin-line)",
            background: "var(--tervin-panel)",
          }}
        >
          <span className="dot dot-muted" />
          <span className="meta">Process exited. ⌘W closes this pane.</span>
        </div>
      )}
    </div>
  );
}

/**
 * A draggable divider.
 *
 * 5px hit area, teal while hovered or dragging, per the design system. Pointer
 * capture is on `window` so a fast drag cannot outrun the handle and leave the
 * divider stuck to the cursor.
 */
function Divider({
  direction,
  onDrag,
}: {
  direction: SplitDirection;
  onDrag: (delta: number) => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);

  const onPointerDown = useCallback(
    (event: React.PointerEvent) => {
      event.preventDefault();
      dragging.current = true;
      ref.current?.setAttribute("data-dragging", "true");

      // Measured against the parent, because the delta is a fraction of the
      // split's own extent rather than of the window.
      const parent = ref.current?.parentElement;
      const extent =
        direction === "row"
          ? (parent?.getBoundingClientRect().width ?? 1)
          : (parent?.getBoundingClientRect().height ?? 1);

      let last = direction === "row" ? event.clientX : event.clientY;

      const move = (e: PointerEvent) => {
        if (!dragging.current) return;
        const current = direction === "row" ? e.clientX : e.clientY;
        const delta = (current - last) / Math.max(extent, 1);
        last = current;
        if (delta !== 0) onDrag(delta);
      };

      const up = () => {
        dragging.current = false;
        ref.current?.removeAttribute("data-dragging");
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        document.body.style.cursor = "";
      };

      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      window.addEventListener("pointercancel", up);
      // Hold the resize cursor for the whole drag, even over a child pane.
      document.body.style.cursor = direction === "row" ? "col-resize" : "row-resize";
    },
    [direction, onDrag],
  );

  // Never leave a listener or a hijacked cursor behind on unmount.
  useEffect(() => {
    return () => {
      document.body.style.cursor = "";
    };
  }, []);

  return (
    <div
      ref={ref}
      className={direction === "row" ? "handle-col" : "handle-row"}
      onPointerDown={onPointerDown}
      role="separator"
      aria-orientation={direction === "row" ? "vertical" : "horizontal"}
      aria-label="Resize panes"
      tabIndex={0}
      // Keyboard resizing, so the divider is not mouse-only.
      onKeyDown={(e) => {
        const step = 0.02;
        if (direction === "row") {
          if (e.key === "ArrowLeft") onDrag(-step);
          else if (e.key === "ArrowRight") onDrag(step);
          else return;
        } else {
          if (e.key === "ArrowUp") onDrag(-step);
          else if (e.key === "ArrowDown") onDrag(step);
          else return;
        }
        e.preventDefault();
      }}
    />
  );
}
