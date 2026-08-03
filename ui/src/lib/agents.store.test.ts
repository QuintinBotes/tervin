/**
 * What the user configured must survive what Tervin failed to find.
 *
 * This file exists because of a specific failure. `agents_overview` returned the
 * user's profiles and the results of probing the machine for installed agents in one
 * call, so a probe that failed failed the whole command — and a user with five
 * profiles in `agents.toml` was shown "No agent profile configured". The profiles
 * were never the problem; they had been read correctly and then thrown away.
 *
 * So these assert the seam rather than the symptom: profiles are set from their own
 * call, and nothing discovery does afterwards can take them back off the screen.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { useWorkspace } from "./store";

function profile(id: string): api.AgentProfile {
  return {
    id,
    name: id,
    runtime_id: "claude-code",
    binary: "claude",
    args: [],
    env: {},
    model: null,
    permission_mode: null,
    badge: null,
    sensitive: false,
  };
}

const OVERVIEW: api.AgentsOverview = {
  profiles: [profile("work"), profile("personal")],
  default_profile: "work",
  launch_options: {
    "claude-code": {
      models: [
        { value: "", label: "Profile default" },
        { value: "opus", label: "Opus" },
      ],
      efforts: [
        { value: "", label: "Default effort" },
        { value: "high", label: "High" },
      ],
    },
  },
  profiles_path: "~/.config/tervin/agents.toml",
  mcp_path: "~/.config/tervin/mcp.json",
};

beforeEach(() => {
  vi.restoreAllMocks();
  useWorkspace.setState({
    agents: null,
    agentsDiscovery: null,
    activeProfileId: null,
    notices: [],
  });
});

describe("refreshAgents", () => {
  it("keeps the configured profiles when discovery fails", async () => {
    vi.spyOn(api, "agentsOverview").mockResolvedValue(OVERVIEW);
    vi.spyOn(api, "agentsDiscovery").mockRejectedValue(
      new Error("$SHELL -ic alias never returned"),
    );

    await useWorkspace.getState().refreshAgents();

    const s = useWorkspace.getState();
    expect(s.agents?.profiles.map((p) => p.id)).toEqual(["work", "personal"]);
    expect(s.activeProfileId).toBe("work");
    // The failure is reported, not swallowed — it just costs nothing above it.
    expect(s.notices.length).toBeGreaterThan(0);
    expect(s.agentsDiscovery).toBeNull();
  });

  it("still asks for discovery when it can succeed", async () => {
    const discovery: api.AgentsDiscovery = { discovered: [], import_candidates: [] };
    vi.spyOn(api, "agentsOverview").mockResolvedValue(OVERVIEW);
    vi.spyOn(api, "agentsDiscovery").mockResolvedValue(discovery);

    await useWorkspace.getState().refreshAgents();

    expect(useWorkspace.getState().agentsDiscovery).toEqual(discovery);
    expect(useWorkspace.getState().agents?.profiles).toHaveLength(2);
  });

  it("drops the model and effort when the profile changes", async () => {
    // A profile can change the runtime, and an alias one runtime resolves is one
    // another may reject or, worse, read as something else. Carrying the old
    // selection across would send it anyway.
    useWorkspace.setState({ activeModel: "opus", activeEffort: "max" });

    useWorkspace.getState().setActiveProfile("personal");

    const s = useWorkspace.getState();
    expect(s.activeProfileId).toBe("personal");
    expect(s.activeModel).toBe("");
    expect(s.activeEffort).toBe("");
  });

  it("does not probe the machine when the profiles themselves could not be read", async () => {
    // Nothing to fill in around, and a second failure would only be noise.
    vi.spyOn(api, "agentsOverview").mockRejectedValue(new Error("agents.toml is malformed"));
    const discovery = vi.spyOn(api, "agentsDiscovery").mockResolvedValue({
      discovered: [],
      import_candidates: [],
    });

    await useWorkspace.getState().refreshAgents();

    expect(discovery).not.toHaveBeenCalled();
    expect(useWorkspace.getState().notices.length).toBeGreaterThan(0);
  });
});
