import { describe, it, expect } from "vitest";
import { pruneToExceptions } from "./pruneToExceptions";
import type { AgentRunNode } from "@/types";

function node(overrides: Partial<AgentRunNode> & { agent_run_id: number }): AgentRunNode {
  return {
    subagent_type: undefined,
    status: "completed",
    tool_call_count: 0,
    children: [],
    ...overrides,
  };
}

describe("pruneToExceptions", () => {
  it("drops a fully successful tree entirely", () => {
    const tree = [
      node({ agent_run_id: 1, children: [node({ agent_run_id: 2 })] }),
      node({ agent_run_id: 3 }),
    ];
    expect(pruneToExceptions(tree)).toEqual([]);
  });

  it("keeps a failed node and prunes its successful siblings", () => {
    const tree = [
      node({ agent_run_id: 1 }),
      node({ agent_run_id: 2, status: "error" }),
    ];
    const result = pruneToExceptions(tree);
    expect(result).toHaveLength(1);
    expect(result[0].agent_run_id).toBe(2);
  });

  it("keeps a successful ancestor of a failed descendant, pruning its OTHER successful children", () => {
    const tree = [
      node({
        agent_run_id: 1,
        children: [
          node({ agent_run_id: 2 }), // successful sibling -- pruned
          node({ agent_run_id: 3, status: "error" }), // kept
        ],
      }),
    ];
    const result = pruneToExceptions(tree);
    expect(result).toHaveLength(1);
    expect(result[0].agent_run_id).toBe(1);
    expect(result[0].children).toHaveLength(1);
    expect(result[0].children[0].agent_run_id).toBe(3);
  });

  it("returns an empty array for an empty tree", () => {
    expect(pruneToExceptions([])).toEqual([]);
  });
});
