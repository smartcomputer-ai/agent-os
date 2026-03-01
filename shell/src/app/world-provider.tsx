import { createContext, useContext } from "react";
import { useHealth, useInfo } from "../sdk/queries";

type WorldInfo = {
  name?: string;
  manifestHash?: string;
  journalHead?: number;
  version?: string;
};

type WorldStatus = {
  connected: boolean;
  label: string;
};

type WorldContextValue = {
  world: WorldInfo;
  status: WorldStatus;
};

const WorldContext = createContext<WorldContextValue | null>(null);

export function WorldProvider({ children }: { children: React.ReactNode }) {
  const healthQuery = useHealth();
  const infoQuery = useInfo();

  const connected = Boolean(healthQuery.data?.ok);
  const label = healthQuery.isLoading
    ? "connecting"
    : connected
      ? "online"
      : "offline";

  const world: WorldInfo = {
    name: infoQuery.data?.world_id ?? undefined,
    manifestHash:
      infoQuery.data?.manifest_hash ?? healthQuery.data?.manifest_hash,
    journalHead: healthQuery.data?.journal_height ?? undefined,
    version: infoQuery.data?.version ?? undefined,
  };

  return (
    <WorldContext.Provider value={{ world, status: { connected, label } }}>
      {children}
    </WorldContext.Provider>
  );
}

export function useWorld() {
  const ctx = useContext(WorldContext);
  if (!ctx) {
    throw new Error("WorldProvider is missing");
  }
  return ctx;
}
