/**
 * `@path` completion for the composer.
 *
 * Attaching a file to a prompt should not mean typing its path from memory, so
 * `@` opens a picker over the project's own file index — gitignore-aware, so it
 * offers source files rather than `node_modules`.
 *
 * The list is keyboard-first and never steals focus: arrows and Enter are handled
 * by the textarea's own key handler and forwarded here, because moving focus into
 * a dropdown mid-sentence loses the caret and the user's place.
 */

import { useEffect, useState } from "react";
import * as api from "../lib/api";

interface Props {
  /** Text after the `@`. */
  query: string;
  /** Scope completion to a directory — a pane's cwd relative to the project. */
  relativeTo?: string | null;
  /** Index of the highlighted row, owned by the caller so keys work in the input. */
  selected: number;
  onCount: (count: number) => void;
  onAccept: (path: string) => void;
}

export function PathComplete({
  query,
  relativeTo,
  selected,
  onCount,
  onAccept,
}: Props) {
  const [results, setResults] = useState<api.Completion[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Debounced so a fast typist does not queue a request per character. The
  // backend reads an in-memory snapshot, so this is about IPC volume rather than
  // filesystem cost.
  useEffect(() => {
    let cancelled = false;
    const handle = setTimeout(() => {
      void api
        .pathComplete(query, "files", relativeTo, 12)
        .then((next) => {
          if (cancelled) return;
          setResults(next);
          setError(null);
          onCount(next.length);
        })
        .catch((e) => {
          if (cancelled) return;
          setResults([]);
          setError(String(e));
          onCount(0);
        });
    }, 60);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [query, relativeTo, onCount]);

  if (error) {
    return (
      <div className="overlay-surface" style={{ padding: "var(--sp-4)" }}>
        <span className="meta tone-amber">Could not complete paths: {error}</span>
      </div>
    );
  }

  if (results.length === 0) {
    return (
      <div className="overlay-surface" style={{ padding: "var(--sp-4)" }}>
        <span className="meta">
          {query
            ? `No file matches “${query}”.`
            : "Type to search the project's files."}
        </span>
      </div>
    );
  }

  return (
    <div
      className="overlay-surface"
      role="listbox"
      aria-label="File completions"
      style={{ maxHeight: 260, overflow: "auto" }}
    >
      {results.map((result, index) => (
        <button
          key={result.path}
          role="option"
          aria-selected={index === selected}
          // `mousedown` rather than `click`: click fires after blur, which would
          // have already closed the picker.
          onMouseDown={(e) => {
            e.preventDefault();
            onAccept(result.path);
          }}
          className="row"
          style={{
            width: "100%",
            padding: "var(--sp-2) var(--sp-6)",
            gap: "var(--sp-2)",
            textAlign: "left",
            background: index === selected ? "var(--tervin-raised)" : "transparent",
            borderLeft:
              index === selected
                ? "2px solid var(--tervin-accent)"
                : "2px solid transparent",
          }}
        >
          <Highlighted text={result.path} positions={result.positions} />
        </button>
      ))}
    </div>
  );
}

/**
 * A path with its matched characters emphasised.
 *
 * Showing *why* a result matched is what makes a fuzzy list trustworthy — without
 * it, a match five directories deep looks like a mistake.
 */
function Highlighted({ text, positions }: { text: string; positions: number[] }) {
  const marks = new Set(positions);
  const chars = [...text];

  // The basename is what the user is looking for, so it carries the ink colour
  // and the directory recedes.
  const lastSlash = text.lastIndexOf("/");

  return (
    <span className="mono truncate" style={{ fontSize: "var(--text-meta)" }}>
      {chars.map((char, index) => (
        <span
          key={index}
          style={{
            color: marks.has(index)
              ? "var(--tervin-accent)"
              : index > lastSlash
                ? "var(--tervin-ink)"
                : "var(--tervin-muted)",
            fontWeight: marks.has(index) ? 600 : 400,
          }}
        >
          {char}
        </span>
      ))}
    </span>
  );
}
