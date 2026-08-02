/**
 * Find in terminal.
 *
 * Sits over the pane rather than in a separate view, because searching is
 * something you do *while* reading output — pushing the terminal aside to search
 * it defeats the purpose.
 *
 * Searching is incremental. Every keystroke re-runs the search from the top, so
 * results track what has been typed, and an in-progress regular expression such
 * as `[` is treated as "no match yet" rather than as an error: the user is
 * mid-thought, not mistaken.
 */

import { useEffect, useRef, useState } from "react";
import { useWorkspace } from "../lib/store";
import { clearSearch, searchInPane, type SearchOptions } from "./TerminalPane";

export function SearchOverlay({ paneId }: { paneId: string | null }) {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState<SearchOptions>({
    regex: false,
    caseSensitive: false,
    wholeWord: false,
  });
  const [found, setFound] = useState<boolean | null>(null);
  const [regexError, setRegexError] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // Incremental: search as the query changes, without moving focus.
  useEffect(() => {
    if (!paneId) return;
    if (!query) {
      clearSearch(paneId);
      setFound(null);
      setRegexError(false);
      return;
    }

    if (options.regex) {
      try {
        new RegExp(query);
        setRegexError(false);
      } catch {
        // Mid-typing, not a failure. Nothing is searched and nothing is shouted.
        setRegexError(true);
        setFound(null);
        return;
      }
    }

    setFound(searchInPane(paneId, query, true, options));
  }, [paneId, query, options]);

  // Leave no highlights behind when the overlay closes.
  useEffect(() => {
    return () => {
      if (paneId) clearSearch(paneId);
    };
  }, [paneId]);

  function step(forward: boolean) {
    if (!paneId || !query) return;
    setFound(searchInPane(paneId, query, forward, options));
  }

  function close() {
    if (paneId) clearSearch(paneId);
    s.setSearch(false);
  }

  const toggle = (key: keyof SearchOptions, label: string, title: string) => (
    <button
      key={key}
      className="btn btn-xs"
      aria-pressed={options[key] ?? false}
      title={title}
      onClick={() => setOptions((o) => ({ ...o, [key]: !o[key] }))}
      style={{
        borderColor: options[key] ? "var(--tervin-accent)" : "var(--tervin-line)",
        color: options[key] ? "var(--tervin-accent)" : "var(--tervin-muted)",
      }}
    >
      {label}
    </button>
  );

  return (
    <div
      role="search"
      className="overlay-surface row"
      style={{
        position: "absolute",
        top: "var(--sp-3)",
        right: "var(--sp-3)",
        zIndex: 60,
        padding: "var(--sp-2) var(--sp-3)",
        gap: "var(--sp-2)",
        width: "min(520px, calc(100% - 24px))",
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          close();
        } else if (e.key === "Enter") {
          e.preventDefault();
          step(!e.shiftKey);
        }
      }}
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={options.regex ? "Regular expression" : "Find in scrollback"}
        aria-label="Find in terminal"
        className="mono grow"
        style={{
          border: "none",
          background: "transparent",
          padding: "0 var(--sp-1)",
          // Red only for a genuine miss, never while a regex is half-typed.
          color:
            found === false && !regexError ? "var(--tervin-red)" : "var(--tervin-ink)",
        }}
      />

      <span className="meta tabular" style={{ minWidth: 62, textAlign: "right" }}>
        {regexError
          ? "…"
          : query === ""
            ? ""
            : found === false
              ? "no match"
              : "match"}
      </span>

      {toggle("caseSensitive", "Aa", "Match case")}
      {toggle("wholeWord", "ab|", "Whole word")}
      {toggle("regex", ".*", "Regular expression")}

      <span style={{ width: 1, height: 18, background: "var(--tervin-line)" }} />

      <button className="btn btn-xs" onClick={() => step(false)} title="Previous (⇧⏎)">
        ↑
      </button>
      <button className="btn btn-xs" onClick={() => step(true)} title="Next (⏎)">
        ↓
      </button>
      <button className="btn btn-xs" onClick={close} title="Close (Esc)">
        ✕
      </button>
    </div>
  );
}
