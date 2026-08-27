import type { TimelineMessage } from "@/features/messages/types";
import { MessageComputerIndicator } from "./MessageComputerIndicator";

export function MessageAgentComputerIndicator({
  message,
}: {
  message: TimelineMessage;
}) {
  return message.isAgent && message.pubkey ? (
    <MessageComputerIndicator agentPubkey={message.pubkey} />
  ) : null;
}
