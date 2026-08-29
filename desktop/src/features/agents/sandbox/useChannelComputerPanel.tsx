import { ComputerPreviewPanel } from "./ComputerPreviewPanel";
import { closeComputerPanel, useComputerPanel } from "./computerPanelStore";

type PanelOptions = {
  isSinglePanelView: boolean;
  split: boolean;
  widthPx: number;
};

export function useChannelComputerPanel(options: PanelOptions) {
  const panel = useComputerPanel();
  const ownerPubkey = panel.open ? panel.ownerPubkey : null;

  return {
    open: Boolean(ownerPubkey),
    surface: ownerPubkey ? (
      <ComputerPreviewPanel
        isSinglePanelView={options.split ? false : options.isSinglePanelView}
        layout={options.split ? "split" : "standalone"}
        onClose={closeComputerPanel}
        openViewerOnReady={panel.viewerRequested}
        ownerPubkey={ownerPubkey}
        transparentChrome={options.split}
        widthPx={options.widthPx}
      />
    ) : null,
  };
}
