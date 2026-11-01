export interface PreparedWorkspaceChange {
  validate: () => Promise<void>;
  apply: () => Promise<void>;
  warnings: string[];
}

export interface WorkspaceChange {
  id: string;
  name: string;
  agent: string;
  destination: string;
  add: boolean;
  prepare: () => Promise<PreparedWorkspaceChange>;
}

export interface ReviewedWorkspaceChange extends WorkspaceChange {
  prepared: PreparedWorkspaceChange;
}

/** Validate the complete review before writing. Each operation still checks its own target. */
export async function applyWorkspaceChanges(changes: ReviewedWorkspaceChange[]) {
  for (const change of changes) await change.prepared.validate();
  const succeeded: string[] = [];
  const failed: { id: string; name: string; error: string }[] = [];

  for (const change of changes) {
    try {
      await change.prepared.apply();
      succeeded.push(change.id);
    } catch (error) {
      failed.push({
        id: change.id,
        name: `${change.name} · ${change.agent}`,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  return { succeeded, failed };
}
