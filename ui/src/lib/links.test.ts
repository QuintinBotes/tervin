import { describe, expect, it } from "vitest";
import { expandSelection, findLinks, pasteNeedsConfirmation } from "./links";

const kinds = (line: string) => findLinks(line).map((l) => l.kind);
const texts = (line: string) => findLinks(line).map((l) => l.text);

describe("findLinks", () => {
  it("finds urls without swallowing sentence punctuation", () => {
    const links = findLinks("see https://example.com/a/b. and more");
    expect(links).toHaveLength(1);
    expect(links[0]!.text).toBe("https://example.com/a/b");
  });

  it("finds a path with line and column as one link", () => {
    // Not a path link plus a stray number: it is one location.
    const links = findLinks("  src/main.rs:42:8: error");
    const file = links.find((l) => l.kind === "file");
    expect(file).toBeDefined();
    expect(file!.path).toBe("src/main.rs");
    expect(file!.line).toBe(42);
    expect(file!.column).toBe(8);
  });

  it("does not treat a clock time as a file location", () => {
    // The regression this guards: "12:30" has the shape of path:line.
    expect(kinds("started at 12:30 today")).not.toContain("file");
  });

  it("does not link ordinary prose that resembles a path", () => {
    expect(findLinks("this and that")).toHaveLength(0);
    expect(findLinks("a sentence about main things")).toHaveLength(0);
  });

  it("links a bare filename only when it has a known extension", () => {
    expect(texts("edit config.toml now")).toContain("config.toml");
    expect(kinds("edit thing now")).not.toContain("file");
  });

  it("links relative and absolute paths", () => {
    expect(texts("wrote ./out/app.js")).toContain("./out/app.js");
    expect(texts("see /etc/hosts for details")).toContain("/etc/hosts");
    expect(texts("open ~/notes/todo.md")).toContain("~/notes/todo.md");
  });

  it("finds local ports but not remote ones", () => {
    const local = findLinks("ready at http://localhost:5173/");
    // The url provider claims the whole address; the port is inside it.
    expect(local.some((l) => l.kind === "url")).toBe(true);

    const bare = findLinks("listening on 127.0.0.1:8080");
    const port = bare.find((l) => l.kind === "port");
    expect(port?.port).toBe(8080);

    // A remote host's port is not something to open.
    expect(kinds("connected to db.internal:5432")).not.toContain("port");
  });

  it("finds python, node, and rustc stack frames with locations", () => {
    const py = findLinks('  File "app/main.py", line 42, in handler');
    expect(py[0]!.kind).toBe("stack-frame");
    expect(py[0]!.path).toBe("app/main.py");
    expect(py[0]!.line).toBe(42);

    const node = findLinks("    at run (/app/src/x.js:10:5)");
    expect(node[0]!.kind).toBe("stack-frame");
    expect(node[0]!.column).toBe(5);

    const rust = findLinks("  --> crates/auth/src/token.rs:211:9");
    expect(rust[0]!.kind).toBe("stack-frame");
    expect(rust[0]!.line).toBe(211);
  });

  it("prefers a stack frame over the bare path inside it", () => {
    // Overlap resolution: one link, not two partial ones.
    const links = findLinks('  File "app/main.py", line 42');
    expect(links).toHaveLength(1);
    expect(links[0]!.kind).toBe("stack-frame");
  });

  it("finds emails", () => {
    expect(texts("contact dev@example.com please")).toContain("dev@example.com");
  });

  it("finds issue ids in both common shapes", () => {
    expect(texts("fixes PROJ-1421 today")).toContain("PROJ-1421");
    expect(texts("closes #4210")).toContain("#4210");
    // An ordinary hyphenated word is not an issue.
    expect(kinds("a well-known 5 thing")).not.toContain("issue");
  });

  it("finds commit hashes but not hex-looking words", () => {
    expect(texts("at commit 9f3a1c7 on main")).toContain("9f3a1c7");
    // All-letters or all-digits runs are not hashes.
    expect(kinds("the deadbeef case")).not.toContain("commit");
    expect(kinds("count 1234567 items")).not.toContain("commit");
  });

  it("never produces overlapping links", () => {
    const line =
      'error at /app/src/x.ts:10:5 see https://example.com/a#L10 or mail dev@example.com PROJ-12 9f3a1c7';
    const links = findLinks(line);
    for (let i = 1; i < links.length; i++) {
      expect(links[i]!.start).toBeGreaterThanOrEqual(links[i - 1]!.end);
    }
  });

  it("returns links in reading order", () => {
    const links = findLinks("see /a/b.rs then https://x.com/y");
    expect(links.map((l) => l.start)).toEqual([...links.map((l) => l.start)].sort((a, b) => a - b));
  });

  it("gives every link an actionable hint", () => {
    for (const link of findLinks("open /a/b.rs:3 and https://x.com and dev@x.com")) {
      expect(link.hint.length).toBeGreaterThan(0);
    }
  });

  it("handles empty and pathological input without hanging", () => {
    expect(findLinks("")).toHaveLength(0);
    // Beyond the scan cap the line still renders; it just carries no links.
    expect(findLinks("a/b.rs ".repeat(2000))).toHaveLength(0);
  });
});

describe("expandSelection", () => {
  it("expands to a whole path rather than a dotted fragment", () => {
    const line = "edit crates/auth/src/token.rs now";
    const index = line.indexOf("token");
    const { start, end } = expandSelection(line, index);
    expect(line.slice(start, end)).toBe("crates/auth/src/token.rs");
  });

  it("expands to a whole url", () => {
    const line = "open https://example.com/a/b?c=1 please";
    const { start, end } = expandSelection(line, line.indexOf("example"));
    expect(line.slice(start, end)).toBe("https://example.com/a/b?c=1");
  });

  it("expands to a word when there is no link", () => {
    const line = "the quick brown fox";
    const { start, end } = expandSelection(line, line.indexOf("quick"));
    expect(line.slice(start, end)).toBe("quick");
  });

  it("selects a single separator when the cursor is on one", () => {
    const line = "a b";
    expect(expandSelection(line, 1)).toEqual({ start: 1, end: 2 });
  });

  it("is safe at the boundaries", () => {
    expect(expandSelection("abc", -1)).toEqual({ start: -1, end: -1 });
    expect(expandSelection("abc", 99)).toEqual({ start: 99, end: 99 });
    expect(expandSelection("", 0)).toEqual({ start: 0, end: 0 });
  });
});

describe("pasteNeedsConfirmation", () => {
  it("confirms a multi-line paste when bracketed paste is off", () => {
    // Each line would run the moment it arrives.
    const result = pasteNeedsConfirmation("git status\nrm -rf build\n", false);
    expect(result.needed).toBe(true);
    expect(result.lines).toBe(2);
    expect(result.reason).toContain("2 lines");
  });

  it("does not confirm when the application enabled bracketed paste", () => {
    // The shell receives it as one unit, so there is nothing to warn about.
    expect(pasteNeedsConfirmation("a\nb\nc\n", true).needed).toBe(false);
  });

  it("treats a single trailing newline as one line", () => {
    // A deliberate "paste and run" is the common case and must not nag.
    expect(pasteNeedsConfirmation("npm test\n", false).needed).toBe(false);
  });

  it("confirms an very large single-line paste", () => {
    const result = pasteNeedsConfirmation("x".repeat(5000), false);
    expect(result.needed).toBe(true);
    expect(result.reason).toContain("characters");
  });

  it("allows an ordinary short paste", () => {
    expect(pasteNeedsConfirmation("ls -la", false).needed).toBe(false);
  });

  it("handles empty input", () => {
    expect(pasteNeedsConfirmation("", false)).toEqual({ needed: false, lines: 0 });
  });
});
