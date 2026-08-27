import * as React from "react";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { ComputerPreviewPanel } from "./ComputerPreviewPanel";
import { closeComputerPanel, useComputerPanel } from "./computerPanelStore";

type PanelOptions = {
  isSinglePanelView: boolean;
  split: boolean;
  widthPx: number;
};

export function useChannelComputerPanel(
  activeChannel: Channel | null,
  currentPubkey: string | undefined,
  options: PanelOptions,
) {
  const panel = useComputerPanel();
  const counterpartPubkey = React.useMemo(() => {
    if (activeChannel?.channelType !== "dm") return null;
    const current = currentPubkey ? normalizePubkey(currentPubkey) : null;
    const others = activeChannel.participantPubkeys.filter(
      (pubkey) => normalizePubkey(pubkey) !== current,
    );
    return others.length === 1 ? others[0] : null;
  }, [activeChannel, currentPubkey]);
  const ownerPubkey =
    panel.open &&
    panel.ownerPubkey &&
    counterpartPubkey &&
    normalizePubkey(panel.ownerPubkey) === normalizePubkey(counterpartPubkey)
      ? panel.ownerPubkey
      : null;

  return {
    open: Boolean(ownerPubkey),
    surface: ownerPubkey ? (
      <ComputerPreviewPanel
        isSinglePanelView={options.split ? false : options.isSinglePanelView}
        layout={options.split ? "split" : "standalone"}
        onClose={closeComputerPanel}
        ownerPubkey={ownerPubkey}
        transparentChrome={options.split}
        widthPx={options.widthPx}
      />
    ) : null,
  };
}
