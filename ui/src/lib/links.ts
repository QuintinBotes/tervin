/**
 * Smart selection: turning terminal text into things you can act on.
 *
 * A terminal prints paths, ports, commit hashes, issue ids, emails and stack
 * frames as plain text. Recognising them is what separates a terminal you read
 * from a terminal you work in.
 *
 * Two rules govern everything here.
 *
 * **Precision over recall.** A wrong link is worse than no link: it opens the
 * wrong file, or turns ordinary prose into a misleading affordance. Every pattern
 * requires structural evidence — a separator, an extension, a known prefix — and
 * prose that merely resembles a path is left alone.
 *
 * **Longest match wins, ties broken by specificity.** `src/main.rs:42:8` is one
 * link to a location, not a path link overlapping a number. Overlaps are resolved
 * once, centrally, so a provider cannot half-cover another provider's match.
 */

/** What a recognised span is. */
export type LinkKind =
  | "url"
  | "file"
  | "port"
  | "commit"
  | "email"
  | "issue"
  | "stack-frame";

export interface LinkMatch {
  kind: LinkKind;
  /** Start index within the line, inclusive. */
  start: number;
  /** End index within the line, exclusive. */
  end: number;
  /** The matched text. */
  text: string;
  /** Resolved path for `file` and `stack-frame`. */
  path?: string;
  line?: number;
  column?: number;
  /** Port number for `port`. */
  port?: number;
  /** What activating this does, shown in a tooltip. */
  hint: string;
}

/**
 * One recogniser.
 *
 * `specificity` breaks ties when two patterns match the same span — a stack frame
 * outranks the bare path inside it.
 */
interface Provider {
  kind: LinkKind;
  specificity: number;
  pattern: RegExp;
  build: (m: RegExpExecArray) => Omit<LinkMatch, "kind" | "start" | "end"> | null;
}

/** Extensions that make a bare word a plausible file without a separator. */
const CODE_EXTENSIONS =
  "rs|ts|tsx|js|jsx|mjs|cjs|py|go|rb|java|kt|swift|c|h|cc|cpp|hpp|cs|php|sh|bash|zsh|fish|sql|toml|yaml|yml|json|jsonc|xml|html|css|scss|md|mdx|txt|log|lock|conf|ini|env|dockerfile|makefile|gradle|proto|graphql|vue|svelte";

const PROVIDERS: Provider[] = [
  // ------------------------------------------------------------------ urls
  {
    kind: "url",
    specificity: 100,
    // Trailing punctuation is excluded so "see https://x.com/a." does not
    // capture the sentence's full stop.
    pattern: /\bhttps?:\/\/[^\s"'<>`|]+[^\s"'<>`|.,;:!?)\]}]/g,
    build: (m) => ({ text: m[0], hint: `Open ${m[0]}` }),
  },

  // ---------------------------------------------------------------- emails
  {
    kind: "email",
    specificity: 90,
    pattern: /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g,
    build: (m) => ({ text: m[0], hint: `Compose to ${m[0]}` }),
  },

  // ----------------------------------------------------------- stack frames
  {
    // Python: `File "app/main.py", line 42`
    kind: "stack-frame",
    specificity: 85,
    pattern: /File "([^"]+)", line (\d+)/g,
    build: (m) => ({
      text: m[0],
      path: m[1],
      line: Number(m[2]),
      hint: `Open ${m[1]} at line ${m[2]}`,
    }),
  },
  {
    // Node/V8: `at fn (/app/src/x.js:10:5)`
    kind: "stack-frame",
    specificity: 85,
    pattern: /\bat\s+[\w.<>[\]$]+\s+\((\/?[\w./\-+@]+):(\d+):(\d+)\)/g,
    build: (m) => ({
      text: m[0],
      path: m[1],
      line: Number(m[2]),
      column: Number(m[3]),
      hint: `Open ${m[1]} at ${m[2]}:${m[3]}`,
    }),
  },
  {
    // rustc: `  --> src/main.rs:10:5`
    kind: "stack-frame",
    specificity: 85,
    pattern: /-->\s+([\w./\-+@]+):(\d+):(\d+)/g,
    build: (m) => ({
      text: m[0],
      path: m[1],
      line: Number(m[2]),
      column: Number(m[3]),
      hint: `Open ${m[1]} at ${m[2]}:${m[3]}`,
    }),
  },

  // ------------------------------------------------------------------ files
  {
    // `path:line[:col]` — the form every toolchain and editor agrees on.
    kind: "file",
    specificity: 70,
    pattern: new RegExp(
      String.raw`(?:^|[\s'"(\[])((?:[~.]{0,2}\/)?[\w./\-+@]*[\w\-+](?:\.(?:${CODE_EXTENSIONS}))?):(\d{1,7})(?::(\d{1,7}))?(?=[\s'")\],:]|$)`,
      "gi",
    ),
    build: (m) => {
      // A bare number with a colon is not a location: require a separator or a
      // known extension, or "12:30" in a log line becomes a file link.
      const path = m[1]!;
      if (!path.includes("/") && !path.includes(".")) return null;
      return {
        text: m[0].trimStart(),
        path,
        line: Number(m[2]),
        column: m[3] ? Number(m[3]) : undefined,
        hint: `Open ${path} at line ${m[2]}`,
      };
    },
  },
  {
    // A bare path with a separator, or a filename with a known extension.
    kind: "file",
    specificity: 50,
    pattern: new RegExp(
      String.raw`(?:^|[\s'"(\[])((?:[~.]{0,2}\/[\w./\-+@]*[\w\-+])|(?:[\w\-+][\w.\-+]*\.(?:${CODE_EXTENSIONS})))(?=[\s'")\],:;]|$)`,
      "gi",
    ),
    build: (m) => ({ text: m[1]!, path: m[1]!, hint: `Open ${m[1]}` }),
  },

  // ------------------------------------------------------------------ ports
  {
    kind: "port",
    specificity: 80,
    // Only local addresses: a port on a remote host is not something to open.
    pattern:
      /\b(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]|::1):(\d{2,5})\b/g,
    build: (m) => {
      const port = Number(m[1]);
      if (port < 1 || port > 65535) return null;
      return {
        text: m[0],
        port,
        hint: `Open http://localhost:${port}`,
      };
    },
  },

  // ----------------------------------------------------------------- issues
  {
    // `ABC-123`, the Jira/Linear shape. Requires uppercase and a hyphen, so an
    // ordinary hyphenated word does not match.
    kind: "issue",
    specificity: 60,
    pattern: /\b([A-Z][A-Z0-9]{1,9}-\d{1,6})\b/g,
    build: (m) => ({ text: m[1]!, hint: `Search for ${m[1]}` }),
  },
  {
    kind: "issue",
    specificity: 60,
    pattern: /(?:^|[\s(])(#\d{1,7})\b/g,
    build: (m) => ({ text: m[1]!, hint: `Open issue ${m[1]}` }),
  },

  // ---------------------------------------------------------------- commits
  {
    // 7–40 hex characters. Shorter would match ordinary words like "added";
    // requiring at least one digit rules out words like "deadbeef" spelled in
    // letters only… which is itself a valid hash, hence the length floor too.
    kind: "commit",
    specificity: 40,
    pattern: /\b([0-9a-f]{7,40})\b/g,
    build: (m) => {
      const text = m[1]!;
      if (!/\d/.test(text)) return null;
      if (!/[a-f]/.test(text)) return null;
      return { text, hint: `Show commit ${text.slice(0, 12)}` };
    },
  },
];

/**
 * Find every actionable span in a line.
 *
 * Overlaps are resolved by preferring the longer match, then the more specific
 * provider, so nested patterns yield one link rather than several partial ones.
 */
export function findLinks(line: string): LinkMatch[] {
  if (!line || line.length > 4000) {
    // A pathologically long line is not worth scanning; the renderer still shows
    // it, it just carries no links.
    return [];
  }

  const candidates: (LinkMatch & { specificity: number })[] = [];

  for (const provider of PROVIDERS) {
    // Providers are module-level and stateful via lastIndex; reset per line.
    provider.pattern.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = provider.pattern.exec(line)) !== null) {
      // A zero-width match would loop forever.
      if (m.index === provider.pattern.lastIndex) provider.pattern.lastIndex++;

      const built = provider.build(m);
      if (!built) continue;

      // Locate the payload inside the whole match, so leading separators
      // captured for context are not part of the link.
      const offset = m[0].indexOf(built.text);
      const start = m.index + (offset >= 0 ? offset : 0);

      candidates.push({
        ...built,
        kind: provider.kind,
        specificity: provider.specificity,
        start,
        end: start + built.text.length,
      });
    }
  }

  // Longest first, then most specific: the winner claims its span.
  candidates.sort(
    (a, b) => b.end - b.start - (a.end - a.start) || b.specificity - a.specificity,
  );

  const claimed: LinkMatch[] = [];
  for (const candidate of candidates) {
    const overlaps = claimed.some((c) => candidate.start < c.end && c.start < candidate.end);
    if (!overlaps) {
      const { specificity: _specificity, ...match } = candidate;
      claimed.push(match);
    }
  }

  return claimed.sort((a, b) => a.start - b.start);
}

/**
 * Expand a selection outward to a meaningful unit.
 *
 * Double-clicking a path should select the whole path, not the fragment between
 * two dots. Word separators are configurable because what counts as one token
 * differs between prose and a shell command line.
 */
export function expandSelection(
  line: string,
  index: number,
  separators = ' \t()[]{}\'"`,;<>|&',
): { start: number; end: number } {
  if (index < 0 || index >= line.length) return { start: index, end: index };

  // A link under the cursor wins: it is already the meaningful unit.
  const link = findLinks(line).find((l) => index >= l.start && index < l.end);
  if (link) return { start: link.start, end: link.end };

  const isSeparator = (ch: string) => separators.includes(ch);
  if (isSeparator(line[index]!)) return { start: index, end: index + 1 };

  let start = index;
  let end = index;
  while (start > 0 && !isSeparator(line[start - 1]!)) start--;
  while (end < line.length && !isSeparator(line[end]!)) end++;
  return { start, end };
}

/**
 * Whether pasted text needs confirmation before it reaches the shell.
 *
 * The danger is not length, it is newlines: a shell executes each line as it
 * arrives, so a multi-line paste runs every line the moment it lands. Bracketed
 * paste removes that risk, which is why an application that has enabled it is
 * exempt.
 */
export function pasteNeedsConfirmation(
  text: string,
  bracketedPasteEnabled: boolean,
): { needed: boolean; reason?: string; lines: number } {
  // A single trailing newline is how a deliberate "run this" paste looks.
  const body = text.endsWith("\n") ? text.slice(0, -1) : text;
  const lines = body.length === 0 ? 0 : body.split("\n").length;

  if (bracketedPasteEnabled) {
    return { needed: false, lines };
  }
  if (lines > 1) {
    return {
      needed: true,
      lines,
      reason: `This paste contains ${lines} lines. Without bracketed paste the shell runs each one as it arrives.`,
    };
  }
  if (text.length > 4096) {
    return {
      needed: true,
      lines,
      reason: `This paste is ${text.length.toLocaleString()} characters.`,
    };
  }
  return { needed: false, lines };
}
