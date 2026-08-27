import type { ChangeEventHandler } from "react";

import { cn } from "@/shared/lib/cn";
import { Input } from "@/shared/ui/input";
import {
  PERSONA_FIELD_CONTROL_CLASS,
  PERSONA_FIELD_SHELL_CLASS,
} from "./agentConfigOptions";

type PersonaJobTitleFieldProps = {
  disabled: boolean;
  onChange: ChangeEventHandler<HTMLInputElement>;
  value: string;
};

export function PersonaJobTitleField({
  disabled,
  onChange,
  value,
}: PersonaJobTitleFieldProps) {
  return (
    <div className="space-y-1.5">
      <label
        className="text-sm font-medium text-foreground"
        htmlFor="persona-job-title"
      >
        Job title{" "}
        <span className="font-normal text-muted-foreground">Optional</span>
      </label>
      <div
        className={cn(
          "flex min-h-11 items-center px-3",
          PERSONA_FIELD_SHELL_CLASS,
        )}
      >
        <Input
          autoCorrect="off"
          className={cn("h-8 px-0 py-0 leading-6", PERSONA_FIELD_CONTROL_CLASS)}
          disabled={disabled}
          id="persona-job-title"
          onChange={onChange}
          placeholder="Principal Researcher"
          value={value}
        />
      </div>
    </div>
  );
}
