import type { AgentRunNode } from "@/types";

/**
 * Collapses successful branches by default, surfacing exceptions first.
 * Keeps a node only if IT is an error or has an error descendant -- an
 * ancestor of a real exception stays
 * visible (with its own OTHER, successful children pruned independently)
 * so the path down to the exception has context, rather than the
 * exception appearing to float with no parent.
 *
 * Lives in its own file (not alongside `AgentRunTreeView`) so exporting
 * it for its own unit test doesn't break that component's Fast Refresh
 * (`react-refresh/only-export-components`).
 */
export function pruneToExceptions(nodes: AgentRunNode[]): AgentRunNode[] {
  const kept: AgentRunNode[] = [];
  for (const node of nodes) {
    const prunedChildren = pruneToExceptions(node.children);
    if (node.status === "error" || prunedChildren.length > 0) {
      kept.push({ ...node, children: prunedChildren });
    }
  }
  return kept;
}
