// Comparing release versions. Used to decide which "what's new" cards a given update should show,
// so getting it wrong means telling someone about features they've had for months — or hiding the
// one thing they just got.

type Parsed = { core: [number, number, number]; prerelease: string | null };

/** Split `1.2.3`, `v1.2.3`, `1.2.3-beta.1`, `1.2` or `1` into comparable parts. Unparseable
 *  segments read as 0, so garbage sorts as the oldest possible version rather than throwing. */
function parse(v: string): Parsed {
  // Build metadata (`+sha`) is ignored for precedence per semver — strip it BEFORE splitting off the
  // prerelease, or `1.0.0+build` reads as a prerelease and sorts below plain `1.0.0`.
  const trimmed = v.trim().replace(/^v/i, "").split("+")[0];
  const [core, ...rest] = trimmed.split("-");
  const nums = core.split(".").map((n) => Number.parseInt(n, 10));
  const at = (i: number) => (Number.isFinite(nums[i]) ? nums[i] : 0);
  return { core: [at(0), at(1), at(2)], prerelease: rest.length ? rest.join("-") : null };
}

/**
 * Standard comparator: negative when `a` is older than `b`, positive when newer, 0 when equal.
 *
 * Numeric per segment, never lexicographic — `0.10.0` is newer than `0.9.0`, which a string compare
 * gets backwards. A prerelease sorts BEFORE its release (`1.0.0-beta` < `1.0.0`), per semver.
 */
export function compareVersions(a: string, b: string): number {
  const pa = parse(a);
  const pb = parse(b);
  for (let i = 0; i < 3; i++) {
    if (pa.core[i] !== pb.core[i]) return pa.core[i] < pb.core[i] ? -1 : 1;
  }
  if (pa.prerelease === pb.prerelease) return 0;
  if (pa.prerelease === null) return 1; // a release outranks its own prerelease
  if (pb.prerelease === null) return -1;
  return pa.prerelease < pb.prerelease ? -1 : 1;
}

/** True when `v` is strictly newer than `than`. */
export function isNewer(v: string, than: string): boolean {
  return compareVersions(v, than) > 0;
}
