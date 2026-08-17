import * as React from "react";

/**
 * The sandbox dock's four app-style icons — the icon IS the tile (no
 * background square behind it, except where the icon's own shape is a
 * rounded square, as in Terminal/Computer). Each is drawn on a 48x48 design
 * grid with whole/half coordinates so it stays crisp at retina.
 *
 * Provenance for every icon (vendored vs. original artwork, and exact
 * upstream source + license for the vendored one) is recorded in
 * `assets/LICENSE-icons.txt`. block/buzz is Apache-2.0, so no GPL-licensed
 * icon theme (e.g. Papirus) is used here — Browser is the real, permissively
 * licensed Chromium logo; the other three are original artwork drawn in a
 * similar flat, two-tone-gradient desktop-icon style without copying any
 * specific theme's files.
 *
 * Every icon's gradient/filter `id`s are namespaced with `React.useId()` —
 * rendering four of these on one page with literal ids would collide and
 * silently break every instance after the first (SVG defs are looked up by
 * id in the document, not scoped to the element that declared them).
 */

const ICON_SIZE_CLASS = "h-full w-full";

/**
 * The official Chromium product logo, vendored verbatim (only whitespace
 * trimmed) from the Chromium source tree — see `assets/LICENSE-icons.txt`
 * for the exact commit path and its BSD-3-Clause license, and
 * `assets/chromium-product-logo.svg` for an unmodified copy of the same
 * markup kept alongside for audit. Chromium, not Chrome, because the
 * sandbox's Browser view runs Debian's `chromium` package.
 */
export function BrowserDockIcon() {
  const id = React.useId();
  return (
    <svg
      viewBox="0 0 256 256"
      fill="none"
      className={ICON_SIZE_CLASS}
      role="img"
      aria-hidden="true"
    >
      <g clipPath={`url(#${id}-clip)`}>
        <path
          d="M128 191.995c35.346 0 64-28.654 64-64 0-35.347-28.654-64-64-64-35.346 0-64 28.654-64 64 0 35.346 28.654 64 64 64Z"
          fill="#fff"
        />
        <path
          d="M96.01 183.41a63.681 63.681 0 0 1-23.42-23.43l-.007.004-55.425-96a128.027 128.027 0 0 0 110.841 192.018l55.436-96.018a63.985 63.985 0 0 1-16.465 18.775 64.007 64.007 0 0 1-70.96 4.651Z"
          fill="#669DF6"
        />
        <path
          d="M191.991 127.984a63.683 63.683 0 0 1-8.581 31.996l.007.004-55.426 96a128.029 128.029 0 0 0 110.872-192H127.991a64 64 0 0 1 64 64Z"
          fill="#AECBFA"
        />
        <path
          d="M128 180c28.719 0 52-23.281 52-52s-23.281-52-52-52-52 23.281-52 52 23.281 52 52 52Z"
          fill="#1A73E8"
        />
        <path
          d="M95.99 72.59a63.684 63.684 0 0 1 32.001-8.566v-.008h110.851A128.035 128.035 0 0 0 127.991.026 128.028 128.028 0 0 0 17.13 63.999l55.436 96.018a64.003 64.003 0 0 1 4.65-70.961A64 64 0 0 1 95.991 72.59Z"
          fill="#1967D2"
        />
      </g>
      <defs>
        <clipPath id={`${id}-clip`}>
          <path fill="#fff" d="M0 0h256v256H0z" />
        </clipPath>
      </defs>
    </svg>
  );
}

/** Original artwork (see `assets/LICENSE-icons.txt`) in a flat,
 *  two-tone-gradient desktop-folder style: the characteristic folded
 *  back-tab silhouette in a paler blue, a richer blue front face with
 *  rounded corners, a soft inner bottom shadow, and a thin top-edge
 *  highlight where the front face catches the light. */
export function FilesDockIcon() {
  const id = React.useId();
  return (
    <svg
      viewBox="0 0 48 48"
      className={ICON_SIZE_CLASS}
      role="img"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-back`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#A9D2FF" />
          <stop offset="100%" stopColor="#7FB8FF" />
        </linearGradient>
        <linearGradient id={`${id}-front`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#63A6FF" />
          <stop offset="100%" stopColor="#2E7BF6" />
        </linearGradient>
        <linearGradient id={`${id}-shadow`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#1547A6" stopOpacity="0" />
          <stop offset="100%" stopColor="#1547A6" stopOpacity="0.4" />
        </linearGradient>
      </defs>

      {/* Folded back-tab silhouette (paler blue) — the notch that reads as
          a hanging-folder tab. */}
      <path
        d="M4.5 13.5a3 3 0 0 1 3-3h9.8a3 3 0 0 1 2.2.95l3 3.25h18a3 3 0 0 1 3 3V19H4.5v-5.5Z"
        fill={`url(#${id}-back)`}
      />

      {/* Front face (deeper blue), rounded corners. */}
      <rect
        x="4.5"
        y="16.5"
        width="39"
        height="22"
        rx="3.5"
        fill={`url(#${id}-front)`}
      />

      {/* Soft inner shadow along the bottom of the front face. */}
      <rect
        x="4.5"
        y="31.5"
        width="39"
        height="7"
        rx="3.5"
        fill={`url(#${id}-shadow)`}
      />

      {/* Thin top-edge highlight where the front face catches the light. */}
      <path
        d="M8 16.5h32a3.5 3.5 0 0 1 3.5 3.5v.6H4.5V20A3.5 3.5 0 0 1 8 16.5Z"
        fill="#ffffff"
        opacity="0.3"
      />
    </svg>
  );
}

/** Original artwork (see `assets/LICENSE-icons.txt`): a near-black
 *  rounded-square tile with a hairline top-edge highlight and a top-left
 *  "&gt;_" prompt in the classic terminal green — the flat desktop-icon
 *  convention for "a terminal," not a trace of any one theme's file. */
export function TerminalDockIcon() {
  const id = React.useId();
  return (
    <svg
      viewBox="0 0 48 48"
      className={ICON_SIZE_CLASS}
      role="img"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-tile`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#3f3f46" />
          <stop offset="12%" stopColor="#18181b" />
          <stop offset="100%" stopColor="#000000" />
        </linearGradient>
      </defs>

      <rect
        x="2"
        y="2"
        width="44"
        height="44"
        rx="10"
        fill={`url(#${id}-tile)`}
      />
      <rect
        x="2.5"
        y="2.5"
        width="43"
        height="43"
        rx="9.5"
        fill="none"
        stroke="#ffffff"
        strokeOpacity="0.12"
      />

      {/* Prompt: ">" chevron plus an underscore cursor, top-left aligned. */}
      <path
        d="M10 15.5 18 21l-8 5.5"
        fill="none"
        stroke="#00E676"
        strokeWidth="2.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <rect x="21" y="25" width="9" height="2.6" rx="1.3" fill="#00E676" />
    </svg>
  );
}

/** Original artwork (see `assets/LICENSE-icons.txt`): a slim-bezel display
 *  icon — dark frame, deep-blue-to-teal "wallpaper" screen, small stand and
 *  base beneath it. */
export function ComputerDockIcon() {
  const id = React.useId();
  return (
    <svg
      viewBox="0 0 48 48"
      className={ICON_SIZE_CLASS}
      role="img"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-bezel`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#3f3f46" />
          <stop offset="100%" stopColor="#18181b" />
        </linearGradient>
        <linearGradient id={`${id}-screen`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#1e3a8a" />
          <stop offset="100%" stopColor="#0d9488" />
        </linearGradient>
        <linearGradient id={`${id}-gloss`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#ffffff" stopOpacity="0.25" />
          <stop offset="100%" stopColor="#ffffff" stopOpacity="0" />
        </linearGradient>
      </defs>

      {/* Bezel. */}
      <rect
        x="3"
        y="5"
        width="42"
        height="27"
        rx="4"
        fill={`url(#${id}-bezel)`}
      />
      {/* Screen, inset from bezel. */}
      <rect
        x="6"
        y="8"
        width="36"
        height="21"
        rx="2"
        fill={`url(#${id}-screen)`}
      />
      <rect
        x="6"
        y="8"
        width="36"
        height="9"
        rx="2"
        fill={`url(#${id}-gloss)`}
      />

      {/* Stand + base. */}
      <rect x="21" y="32" width="6" height="6" fill="#27272a" />
      <rect x="14" y="38" width="20" height="3" rx="1.5" fill="#3f3f46" />
    </svg>
  );
}
