/** True when a scroll container is within `threshold` pixels of its end (also true when nothing overflows). */
export function isNearEnd(
  { scrollHeight, scrollTop, clientHeight }: { scrollHeight: number; scrollTop: number; clientHeight: number },
  threshold = 200,
) {
  return scrollHeight - scrollTop - clientHeight < threshold;
}
