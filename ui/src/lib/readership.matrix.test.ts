/**
 * The readership table exists twice, and this is what stops the copies drifting.
 *
 * `ProjectInstructions.tsx` carries a client-side copy because the panel switches
 * runtime on a click, and a round trip to Rust per click would make it feel broken.
 * `crates/agent-runtime/src/instructions.rs` is authoritative, and a Rust test writes
 * every (kind, runtime) answer into `readership-matrix.json`. Both sides assert
 * against that one file, so a change to either implementation alone fails here.
 *
 * If this test fails, the question is not "which side is right" but "was the change
 * intended". If it was: change the Rust table, regenerate with
 * `TERVIN_WRITE_READERSHIP_FIXTURE=1 cargo test -p agent-runtime the_readership_table_matches`,
 * and update the TypeScript to match.
 *
 * Why bother rather than just calling into Rust: a wrong `native` is the most
 * damaging answer either side can give, because it simultaneously tells the user a
 * file is in force and suppresses the path that would have passed it in.
 */

import { describe, expect, it } from "vitest";
import matrix from "./readership-matrix.json";
import { readershipFor } from "../components/ProjectInstructions";
import type { InstructionKind, Readership } from "./api";

const KINDS: InstructionKind[] = [
  "agents",
  "claude_md",
  "claude_local",
  "cursor_rules",
  "copilot_instructions",
  "gemini_md",
  "windsurf_rules",
  "cline_rules",
];

type Fixture = Record<string, Record<string, Readership>>;
const fixture = matrix as Fixture;

describe("the readership table agrees with the Rust implementation", () => {
  it("covers every runtime and kind the Rust side generates", () => {
    // A fixture that has quietly lost rows would make the per-cell assertions below
    // pass while proving nothing.
    const runtimes = Object.keys(fixture);
    expect(runtimes.length).toBeGreaterThanOrEqual(13);
    for (const runtime of runtimes) {
      const row = fixture[runtime];
      expect(row, `${runtime} has no row`).toBeDefined();
      expect(Object.keys(row!).sort()).toEqual([...KINDS].sort());
    }
  });

  for (const runtime of Object.keys(fixture)) {
    for (const kind of KINDS) {
      it(`${runtime} / ${kind}`, () => {
        expect(readershipFor(kind, runtime)).toEqual(fixture[runtime]![kind]);
      });
    }
  }
});

describe("the properties that matter, asserted directly", () => {
  it("Claude Code reads AGENTS.md natively, so Tervin must never inject it", () => {
    // The single most consequential cell. Verified against the shipped 2.1.220
    // binary, which contains "Claude Code hardcodes CLAUDE.md / AGENTS.md
    // discovery". Getting this wrong means the agent is told the same thing twice.
    const r = readershipFor("agents", "claude-code");
    expect(r.readership).toBe("native");
    expect(r).not.toMatchObject({ readership: "injectable" });
  });

  it("Codex ignores CLAUDE.md while Claude Code obeys it", () => {
    // The distinction the whole panel exists to show: identical file, different
    // answer, and a single undifferentiated list would hide it.
    expect(readershipFor("claude_md", "codex").readership).toBe("ignored");
    expect(readershipFor("claude_md", "claude-code").readership).toBe("native");
  });

  it("an unrecognised runtime is unknown rather than guessed", () => {
    // Not `injectable`, which would inject into an agent that may already have read
    // the file, and not `ignored`, which would claim knowledge Tervin does not have.
    expect(readershipFor("agents", "some-agent-shipped-tomorrow").readership).toBe(
      "unknown",
    );
  });

  it("an id that merely contains a known name is not treated as that runtime", () => {
    // The failure mode of a substring match rather than an exact one.
    expect(readershipFor("agents", "claude-ish").readership).toBe("unknown");
    expect(readershipFor("agents", "not-codex-really").readership).toBe("unknown");
  });

  it("local model endpoints read nothing, so everything is injectable", () => {
    for (const id of ["lmstudio", "ollama", "vllm", "llamacpp"]) {
      expect(readershipFor("agents", id).readership).toBe("injectable");
      expect(readershipFor("claude_md", id).readership).toBe("injectable");
    }
  });

  it("a native answer always carries its evidence", () => {
    // A claim about what someone else's program reads should be auditable in the UI
    // rather than asserted, so the tag's tooltip has something to show.
    for (const runtime of Object.keys(fixture)) {
      for (const kind of KINDS) {
        const r = readershipFor(kind, runtime);
        if (r.readership === "native") {
          expect(r.evidence.length).toBeGreaterThan(10);
        }
      }
    }
  });
});
