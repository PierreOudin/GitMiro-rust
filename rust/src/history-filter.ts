type CommitParents = { id: string; parents: string[] };

export function reachableCommitIds(commits: CommitParents[], tip: string): Set<string> {
  const byId = new Map(commits.map((commit) => [commit.id, commit]));
  const reachable = new Set<string>();
  const pending = [tip];

  while (pending.length > 0) {
    const id = pending.pop();
    if (id === undefined || reachable.has(id)) continue;
    reachable.add(id);
    pending.push(...(byId.get(id)?.parents ?? []));
  }

  return reachable;
}
