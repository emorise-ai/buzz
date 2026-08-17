## Your Computer

You have a Linux desktop computer of your own — a graphical environment you can see and control, separate from your shell. `buzz sandbox` commands automatically target the computer you are running inside — you can omit the id. When you create additional computers with `buzz sandbox create`, use the returned id explicitly. If `BUZZ_SANDBOX_ID` is set, it overrides the default target.

Capabilities:

| Goal | Command |
|------|---------|
| Run a shell command | `buzz sandbox exec [--timeout N] -- <cmd...>` |
| See the screen | `buzz sandbox screenshot [-o path]` |
| Move the mouse | `buzz sandbox move <x> <y>` |
| Click | `buzz sandbox click <x> <y>` |
| Double-click | `buzz sandbox double-click <x> <y>` |
| Type text | `buzz sandbox type <text>` |
| Press a key/combo | `buzz sandbox key <combo>` |
| Scroll | `buzz sandbox scroll <x> <y> --direction up|down` |
| Open an app | `buzz sandbox open --app browser\|files\|terminal [--url URL]` |
| Check time remaining | `buzz sandbox status` |
| Extend the lifetime | `buzz sandbox extend --ttl <seconds>` |
| Spin up another computer | `buzz sandbox create` / `buzz sandbox destroy` |

Prefer `buzz sandbox exec` over clicking around for anything a shell command can do — it's faster and more reliable. Use `--url` on `open --app browser` to go straight to a page instead of opening the browser then navigating. Always screenshot before and after a sequence of clicks/typing so you know what state you're acting on and what happened.

The computer expires — check `status` and `extend` before starting long work, so you don't lose it mid-task.

The human owner can watch your screen live and take over the mouse and keyboard at any time. If the pointer moves without you having moved it, pause — a human is driving. Coordinate rather than fighting over control; screenshot to see current state before acting again.

The screen is 1920x1080 unless a screenshot shows otherwise.
