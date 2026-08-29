import { LogIn, Monitor, SquareTerminal } from "lucide-react";
import type * as React from "react";

import { ChatHeader } from "@/features/chat/ui/ChatHeader";
import type { EphemeralChannelDisplay } from "@/features/channels/lib/ephemeralChannel";
import type { ActiveDmHeaderParticipant } from "@/features/channels/useActiveChannelHeader";
import { getChannelDescription } from "@/features/channels/lib/channelDescription";
import { getDmParticipantPreview } from "@/features/channels/lib/dmParticipantDisplay";
import { ChannelGlyph } from "@/features/channels/ui/ChannelGlyph";
import { ChannelHeaderStatusBadge } from "@/features/channels/ui/ChannelHeaderStatusBadge";
import { ChannelMembersBar } from "@/features/channels/ui/ChannelMembersBar";
import { useAgentSandbox } from "@/features/agents/sandbox/useAgentSandbox";
import { useHasEverHadComputer } from "@/features/agents/sandbox/computerEverAssignedStore";
import {
  toggleComputerPanel,
  useComputerPanel,
} from "@/features/agents/sandbox/computerPanelStore";
import { startAndOpenComputer } from "@/features/agents/sandbox/sandboxLifecycle";
import { toast } from "sonner";
import {
  DEFAULT_HOVER_PROFILE_STATUS_GEOMETRY,
  ProfileAvatarWithStatus,
  scaleProfileAvatarStatusGeometry,
} from "@/features/profile/ui/ProfileAvatarWithStatus";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { Button } from "@/shared/ui/button";
import type { Channel, PresenceStatus } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { UserAvatar } from "@/shared/ui/UserAvatar";
import {
  toggleTerminalPanel,
  useTerminalPanel,
} from "@/features/terminal/terminalPanelStore";

const DM_HEADER_AVATAR_SIZE = 32;
const DM_HEADER_AVATAR_STATUS_GEOMETRY = scaleProfileAvatarStatusGeometry(
  DEFAULT_HOVER_PROFILE_STATUS_GEOMETRY,
  DM_HEADER_AVATAR_SIZE,
);

type ChannelScreenHeaderProps = {
  activeChannel: Channel | null;
  activeChannelEphemeralDisplay: EphemeralChannelDisplay | null;
  activeChannelTitle: string;
  actionsVariant?: "inline" | "compact";
  activeDmAvatarUrl: string | null;
  activeDmHeaderParticipants: ActiveDmHeaderParticipant[];
  activeDmPresenceStatus: PresenceStatus | null;
  chromeWrapperRef?: React.Ref<HTMLDivElement>;
  currentPubkey?: string;
  headerEndActions?: React.ReactNode;
  isAddBotOpen?: boolean;
  isJoining?: boolean;
  showHeaderContent?: boolean;
  transparentChrome?: boolean;
  onAddBotOpenChange?: (open: boolean) => void;
  onJoinChannel?: () => Promise<void>;
  onManageChannel: () => void;
  onToggleMembers: () => void;
};

export function ChannelScreenHeader({
  activeChannel,
  activeChannelEphemeralDisplay,
  activeChannelTitle,
  actionsVariant = "inline",
  activeDmAvatarUrl,
  activeDmHeaderParticipants,
  activeDmPresenceStatus,
  chromeWrapperRef,
  currentPubkey,
  headerEndActions,
  isAddBotOpen,
  isJoining = false,
  onAddBotOpenChange,
  showHeaderContent = true,
  transparentChrome = false,
  onJoinChannel,
  onManageChannel,
  onToggleMembers,
}: ChannelScreenHeaderProps) {
  const isGroupDm =
    activeChannel?.channelType === "dm" &&
    activeDmHeaderParticipants.length > 1;
  const activeDmParticipant = activeDmHeaderParticipants[0] ?? null;
  const activeDmJobTitle =
    !isGroupDm && activeDmParticipant?.jobTitle
      ? activeDmParticipant.jobTitle
      : undefined;
  const showJoinButton =
    activeChannel !== null &&
    !activeChannel.isMember &&
    activeChannel.visibility === "open" &&
    !activeChannel.archivedAt &&
    onJoinChannel;

  const terminalPanel = useTerminalPanel();
  const terminalButton = activeChannel ? (
    <Button
      aria-label={
        terminalPanel.mode === "closed" ? "Open Buzz Term" : "Hide Buzz Term"
      }
      onClick={toggleTerminalPanel}
      size="icon"
      title="Buzz Term (⌘J)"
      type="button"
      variant={terminalPanel.mode === "closed" ? "outline" : "secondary"}
    >
      <SquareTerminal />
    </Button>
  ) : null;

  // Only a 1:1 DM has a single "the agent I'm talking to" — group DMs and
  // channels have no unambiguous counterpart to show a computer for.
  const dmCounterpartPubkey =
    activeChannel?.channelType === "dm" &&
    activeDmHeaderParticipants.length === 1
      ? activeDmHeaderParticipants[0].pubkey
      : null;
  const dmCounterpartSandbox = useAgentSandbox(dmCounterpartPubkey);
  const dmCounterpartEverHadComputer =
    useHasEverHadComputer(dmCounterpartPubkey);
  const computerPanel = useComputerPanel();
  const isComputerPanelOpenForCounterpart =
    computerPanel.open &&
    dmCounterpartPubkey !== null &&
    computerPanel.ownerPubkey !== null &&
    normalizePubkey(computerPanel.ownerPubkey) ===
      normalizePubkey(dmCounterpartPubkey);
  // Three states: never had a computer → no button; had one but it's gone
  // (expired/stopped) → dim "asleep" button that starts a new one; live →
  // bright button that opens the existing preview.
  const computerButton =
    activeChannel && dmCounterpartPubkey && dmCounterpartSandbox ? (
      <Button
        data-testid="channel-computer-button"
        aria-label={
          isComputerPanelOpenForCounterpart ? "Hide computer" : "Open computer"
        }
        onClick={() => toggleComputerPanel(dmCounterpartPubkey)}
        size="icon"
        title="Computer"
        type="button"
        variant={isComputerPanelOpenForCounterpart ? "secondary" : "outline"}
      >
        <Monitor />
      </Button>
    ) : activeChannel && dmCounterpartPubkey && dmCounterpartEverHadComputer ? (
      <Button
        aria-label="Start computer"
        className="opacity-50 hover:opacity-80"
        data-testid="channel-computer-button"
        onClick={() => {
          startAndOpenComputer(dmCounterpartPubkey).catch((err) => {
            console.error(
              "[ChannelScreenHeader] startAndOpenComputer failed:",
              err,
            );
            toast.error(
              err instanceof Error
                ? err.message
                : "Could not start a computer.",
            );
          });
        }}
        size="icon"
        title="Computer (asleep)"
        type="button"
        variant="outline"
      >
        <Monitor />
      </Button>
    ) : null;
  const channelActions = activeChannel ? (
    showJoinButton ? (
      <div className="flex items-center gap-1">
        <Button
          disabled={isJoining}
          onClick={() => void onJoinChannel()}
          size="sm"
          variant="default"
        >
          <LogIn className="mr-1.5 h-4 w-4" />
          {isJoining ? "Joining…" : "Join"}
        </Button>
        {headerEndActions}
      </div>
    ) : (
      <ChannelMembersBar
        channel={activeChannel}
        currentPubkey={currentPubkey}
        endActions={headerEndActions}
        isAddBotOpen={isAddBotOpen}
        onAddBotOpenChange={onAddBotOpenChange}
        onManageChannel={onManageChannel}
        onToggleMembers={onToggleMembers}
        variant={actionsVariant}
      />
    )
  ) : (
    headerEndActions
  );
  const actions =
    computerButton || terminalButton || channelActions ? (
      <div className="flex items-center gap-1">
        {computerButton}
        {terminalButton}
        {channelActions}
      </div>
    ) : null;

  if (!showHeaderContent) {
    return null;
  }

  return (
    <ChatHeader
      belowSystemChrome
      chromeWrapperRef={chromeWrapperRef}
      actions={actions}
      channelType={activeChannel?.channelType}
      description={getChannelDescription(activeChannel)}
      leadingContent={
        activeChannel?.channelType === "dm" ? (
          isGroupDm ? (
            <DmHeaderParticipantStack
              participants={activeDmHeaderParticipants}
            />
          ) : activeDmParticipant ? (
            <UserProfilePopover
              pubkey={activeDmParticipant.pubkey}
              triggerAriaLabel={`Open profile for ${activeChannelTitle}`}
              triggerElement="span"
            >
              <ProfileAvatarWithStatus
                avatarClassName="text-xs"
                avatarUrl={activeDmAvatarUrl}
                className="mr-1.5 h-8 w-8"
                geometry={DM_HEADER_AVATAR_STATUS_GEOMETRY}
                iconClassName="h-4 w-4"
                label={activeChannelTitle}
                size={DM_HEADER_AVATAR_SIZE}
                status={activeDmPresenceStatus ?? "offline"}
                statusTestId="chat-presence-badge"
                testId="chat-header-dm-avatar"
              />
            </UserProfilePopover>
          ) : (
            <ProfileAvatarWithStatus
              avatarClassName="text-xs"
              avatarUrl={activeDmAvatarUrl}
              className="mr-1.5 h-8 w-8"
              geometry={DM_HEADER_AVATAR_STATUS_GEOMETRY}
              iconClassName="h-4 w-4"
              label={activeChannelTitle}
              size={DM_HEADER_AVATAR_SIZE}
              status={activeDmPresenceStatus ?? "offline"}
              statusTestId="chat-presence-badge"
              testId="chat-header-dm-avatar"
            />
          )
        ) : activeChannel ? (
          <ChannelGlyph
            channel={activeChannel}
            className="h-4 w-4 translate-y-px text-muted-foreground"
          />
        ) : undefined
      }
      statusBadge={
        <ChannelHeaderStatusBadge
          ephemeralDisplay={activeChannelEphemeralDisplay}
        />
      }
      subtitle={activeDmJobTitle}
      title={activeChannelTitle}
      transparentChrome={transparentChrome}
      visibility={activeChannel?.visibility}
    />
  );
}

function DmHeaderParticipantStack({
  participants,
}: {
  participants: ActiveDmHeaderParticipant[];
}) {
  const { hiddenCount, visibleParticipants } =
    getDmParticipantPreview(participants);
  const stackItemCount = visibleParticipants.length + (hiddenCount > 0 ? 1 : 0);

  return (
    <div
      className="mr-1.5 flex shrink-0 items-center"
      data-testid="chat-header-dm-avatar-stack"
    >
      {visibleParticipants.map((participant, index) => (
        <UserProfilePopover
          key={participant.pubkey}
          pubkey={participant.pubkey}
          triggerAriaLabel={`Open profile for ${participant.displayName}`}
          triggerElement="span"
        >
          <span
            className={index > 0 ? "-ml-2" : ""}
            data-testid="chat-header-dm-avatar-stack-participant"
            style={{
              zIndex: index + 1,
              ...(index < stackItemCount - 1 && {
                mask: "radial-gradient(circle 18px at calc(100% + 4px) 50%, transparent 99%, #fff 100%)",
                WebkitMask:
                  "radial-gradient(circle 18px at calc(100% + 4px) 50%, transparent 99%, #fff 100%)",
              }),
            }}
          >
            <UserAvatar
              avatarUrl={participant.avatarUrl}
              className="h-8 w-8 text-xs"
              displayName={participant.displayName}
              size="sm"
            />
          </span>
        </UserProfilePopover>
      ))}
      {hiddenCount > 0 ? (
        <div
          className={visibleParticipants.length > 0 ? "-ml-2" : ""}
          data-testid="chat-header-dm-avatar-stack-more"
          style={{ zIndex: stackItemCount }}
        >
          <span className="flex h-8 w-8 items-center justify-center rounded-full bg-secondary font-semibold text-secondary-foreground shadow-xs">
            <span className="text-2xs leading-none">+{hiddenCount}</span>
          </span>
        </div>
      ) : null}
    </div>
  );
}
