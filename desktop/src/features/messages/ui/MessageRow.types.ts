import type { TimelineMessage } from "@/features/messages/types";

export type ThreadDepthGuideAction = {
  active?: boolean;
  depth: number;
  label: string;
  message: TimelineMessage;
};
