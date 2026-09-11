/** How fresh the data is, and the one button that makes it fresher. */

import { useState } from "react";
import { api, errorMessage } from "../api";
import { useResource } from "../hooks";
import { today } from "../lib/dates";
import { resetDayIndex } from "../lib/dayIndex";
import { formatDayShort, formatRelative } from "../lib/format";

export default function StatusLine({ onIngested }: { onIngested: () => void }) {
  const status = useResource("status", () => api.status());
  const [running, setRunning] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  async function refresh() {
    setRunning(true);
    setFailure(null);
    try {
      await api.ingest();
      // A pass can bring in days that were not there before, so the navigation index is stale.
      resetDayIndex();
      status.reload();
      onIngested();
    } catch (error) {
      setFailure(errorMessage(error));
    } finally {
      setRunning(false);
    }
  }

  if (failure) {
    return <p className="statusline statusline-error">Ingest failed. {failure}</p>;
  }
  if (!status.data) {
    return <p className="statusline">{status.error ? errorMessage(status.error) : ""}</p>;
  }

  const backup = status.data.lastBackupDate;
  const ingested = status.data.lastRun?.finishedAt ?? null;
  return (
    <p className="statusline">
      <span>
        {/* The backup happened on this machine's clock, so this one date is legitimately local. */}
        {backup ? `Backup from ${formatDayShort(today(new Date(backup)))}` : "No backup seen yet"},
        ingested {formatRelative(ingested)}
      </span>
      <button type="button" className="link-button" onClick={refresh} disabled={running}>
        {running ? "Ingesting…" : "Refresh"}
      </button>
    </p>
  );
}
