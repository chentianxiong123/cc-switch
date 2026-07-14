import { useQuery } from "@tanstack/react-query";
import { providersApi } from "@/lib/api";

export const piAgentKeys = {
  all: ["pi-agent"] as const,
  liveProviderIds: ["pi-agent", "liveProviderIds"] as const,
};

export function usePiAgentLiveProviderIds(enabled: boolean) {
  return useQuery({
    queryKey: piAgentKeys.liveProviderIds,
    queryFn: () => providersApi.getPiAgentLiveProviderIds(),
    enabled,
  });
}
