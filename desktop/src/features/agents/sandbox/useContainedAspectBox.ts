import * as React from "react";

export type Box = { width: number; height: number };

/**
 * The largest box of `aspectRatio` (width/height) that fits inside a
 * `containerWidth` x `containerHeight` area — the same "letterbox to fit"
 * math as CSS `object-fit: contain`. Pure so it can be unit tested without a
 * DOM; the hook below supplies live measurements from a ResizeObserver.
 */
export function containedAspectBox(
  containerWidth: number,
  containerHeight: number,
  aspectRatio: number,
): Box | null {
  if (containerWidth <= 0 || containerHeight <= 0) return null;
  const fitByWidth = {
    width: containerWidth,
    height: containerWidth / aspectRatio,
  };
  if (fitByWidth.height <= containerHeight) return fitByWidth;
  return { width: containerHeight * aspectRatio, height: containerHeight };
}

/**
 * Live version of `containedAspectBox`, tracking a container element's size.
 * A flex/grid item cannot derive "shrink to fit on whichever axis is the
 * bottleneck" from CSS `aspect-ratio` alone — that only ties height to
 * width, never to the container's height too — so this measures and
 * recomputes on resize instead.
 *
 * Takes the element itself (from a state-backed callback ref), not a
 * RefObject: the stage lives inside dialog content that mounts only when
 * the dialog opens, so a RefObject would still be null on the hook's first
 * run and the effect would never re-fire when the element appears.
 *
 * Returns `null` until the container has been measured once.
 */
export function useContainedAspectBox(
  container: HTMLElement | null,
  aspectRatio: number,
): Box | null {
  const [box, setBox] = React.useState<Box | null>(null);

  React.useEffect(() => {
    if (!container) return;

    const measure = () => {
      const { width, height } = container.getBoundingClientRect();
      const next = containedAspectBox(width, height, aspectRatio);
      if (next) setBox(next);
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, [container, aspectRatio]);

  return box;
}
