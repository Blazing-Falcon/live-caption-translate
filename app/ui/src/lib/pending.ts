/**
 * A setting that needs a model is saved only once that model is ready. Saving it first would
 * restart listening without the model, and listening would stop until the download ends.
 */
import { get, writable, type Readable } from "svelte/store";
import * as api from "./api";
import type { Session } from "./events";
import type { ConfigPatch } from "./settings";

export interface PendingChange {
  model: string;
  key: string;
  patch: ConfigPatch;
}

export interface PendingChanges extends Readable<readonly PendingChange[]> {
  /** Downloads `model` if needed and saves `patch` when it is ready. */
  request(change: PendingChange): Promise<void>;
  cancel(model: string): void;
  dispose(): void;
}

export function createPendingChanges(session: Session): PendingChanges {
  const pending = writable<readonly PendingChange[]>([]);

  async function settle(): Promise<void> {
    const rows = get(session.models) ?? [];
    for (const change of get(pending)) {
      if (rows.find((row) => row.id === change.model)?.state !== "ready") continue;
      pending.update((list) => list.filter((item) => item !== change));
      await session.save(change.key, change.patch);
    }
  }

  const stop = session.models.subscribe(() => void settle());

  return {
    subscribe: pending.subscribe,
    async request(change) {
      pending.update((list) => [...list.filter((item) => item.model !== change.model), change]);
      const row = get(session.models)?.find((model) => model.id === change.model);
      if (row?.state === "ready") return settle();
      if (row?.state === "downloading" || row?.state === "verifying") return;
      // A command without a result resolves to null, so success is marked explicitly.
      const started = await session.act(`models.${change.model}`, async () => {
        await api.modelsDownloadOptional(change.model);
        return true;
      });
      if (started === null) pending.update((list) => list.filter((item) => item !== change));
    },
    cancel(model) {
      pending.update((list) => list.filter((item) => item.model !== model));
    },
    dispose: stop,
  };
}
