import * as React from "react";
import { KeyRound, Loader2, RotateCw } from "lucide-react";
import { toast } from "sonner";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  restartManagedAgentRuntime,
  setManagedAgentCredential,
} from "@/shared/api/tauriManagedAgents";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";

type SecureAgentCredentialDialogProps = {
  agentName: string;
  agentPubkey: string;
  envKey: string;
  onOpenChange: (open: boolean) => void;
  open: boolean;
};

export function SecureAgentCredentialDialog({
  agentName,
  agentPubkey,
  envKey,
  onOpenChange,
  open,
}: SecureAgentCredentialDialogProps) {
  const { activeCommunity } = useCommunities();
  const inputId = React.useId();
  const [value, setValue] = React.useState("");
  const [phase, setPhase] = React.useState<
    "idle" | "saving" | "restart-failed" | "retrying"
  >("idle");
  const [saveError, setSaveError] = React.useState(false);
  const [retryError, setRetryError] = React.useState(false);

  const reset = React.useCallback(() => {
    setValue("");
    setPhase("idle");
    setSaveError(false);
    setRetryError(false);
  }, []);

  const handleOpenChange = (nextOpen: boolean) => {
    if (phase === "saving" || phase === "retrying") return;
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  };

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!value.trim() || !activeCommunity) return;

    setPhase("saving");
    setSaveError(false);
    try {
      const result = await setManagedAgentCredential({
        pubkey: agentPubkey,
        key: envKey,
        value,
      });
      setValue("");
      if (result.restartError) {
        setPhase("restart-failed");
        return;
      }
      toast.success(`${agentName} restarted with the new credential.`);
      reset();
      onOpenChange(false);
    } catch {
      setPhase("idle");
      setSaveError(true);
    }
  };

  const handleRetryRestart = async () => {
    if (!activeCommunity) return;
    setPhase("retrying");
    setRetryError(false);
    try {
      await restartManagedAgentRuntime(agentPubkey, activeCommunity.relayUrl);
      toast.success(`${agentName} restarted.`);
      reset();
      onOpenChange(false);
    } catch {
      setPhase("restart-failed");
      setRetryError(true);
    }
  };

  const isBusy = phase === "saving" || phase === "retrying";
  const showRestartPanel = phase === "restart-failed" || phase === "retrying";

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent
        aria-describedby="secure-agent-credential-description"
        className="max-w-md"
        data-testid="secure-agent-credential-dialog"
      >
        <DialogHeader>
          <div className="mb-1 flex h-10 w-10 items-center justify-center rounded-full bg-muted text-foreground">
            <KeyRound aria-hidden="true" className="h-5 w-5" />
          </div>
          <DialogTitle>Add a credential for {agentName}</DialogTitle>
          <DialogDescription id="secure-agent-credential-description">
            Enter it here. Do not paste API keys or tokens into chat. Buzz will
            add it only to this agent's environment and restart the agent.
          </DialogDescription>
        </DialogHeader>

        {showRestartPanel ? (
          <div
            className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm"
            data-testid="credential-restart-failed"
          >
            <p className="font-medium text-foreground">
              Credential saved, but {agentName} could not restart.
            </p>
            <p className="mt-1 text-muted-foreground">
              You do not need to enter it again. Retry the restart when ready.
            </p>
            {retryError ? (
              <p className="mt-2 text-destructive" role="alert">
                The restart retry failed. The credential is still saved; try
                again when the runtime is available.
              </p>
            ) : null}
          </div>
        ) : (
          <form className="contents" onSubmit={handleSubmit}>
            <div className="space-y-2">
              <p className="text-sm font-medium">Environment key</p>
              <code className="block rounded-md bg-muted px-3 py-2 font-mono text-sm">
                {envKey}
              </code>
              <label className="text-sm font-medium" htmlFor={inputId}>
                Secret value
              </label>
              <Input
                autoComplete="new-password"
                autoFocus
                data-testid="secure-agent-credential-input"
                disabled={isBusy}
                id={inputId}
                onChange={(event) => setValue(event.target.value)}
                placeholder="Paste the credential here"
                spellCheck={false}
                type="password"
                value={value}
              />
              {saveError ? (
                <p className="text-sm text-destructive" role="alert">
                  Buzz could not save this credential. Check the value and try
                  again.
                </p>
              ) : null}
            </div>
            <DialogFooter>
              <Button
                disabled={isBusy}
                onClick={() => handleOpenChange(false)}
                type="button"
                variant="outline"
              >
                Cancel
              </Button>
              <Button disabled={isBusy || !value.trim()} type="submit">
                {phase === "saving" ? (
                  <Loader2
                    aria-hidden="true"
                    className="mr-2 h-4 w-4 animate-spin"
                  />
                ) : null}
                Save and restart
              </Button>
            </DialogFooter>
          </form>
        )}

        {showRestartPanel ? (
          <DialogFooter>
            <Button
              disabled={isBusy}
              onClick={() => handleOpenChange(false)}
              type="button"
              variant="outline"
            >
              Close
            </Button>
            <Button
              disabled={isBusy}
              onClick={() => void handleRetryRestart()}
              type="button"
            >
              {phase === "retrying" ? (
                <Loader2
                  aria-hidden="true"
                  className="mr-2 h-4 w-4 animate-spin"
                />
              ) : (
                <RotateCw aria-hidden="true" className="mr-2 h-4 w-4" />
              )}
              Retry restart
            </Button>
          </DialogFooter>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
