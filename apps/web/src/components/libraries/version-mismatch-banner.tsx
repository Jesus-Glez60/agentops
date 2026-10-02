import { useRouter } from "next/navigation";
import type { RepoLibraryUsage } from "@/lib/api/libraries-api";

/** Shown when at least one repo's declared version differs from the version currently being viewed -- not necessarily the same set as `library.has_mismatch` (that's always against the latest indexed version; this compares against whatever version the detail page's selector is showing). */
export function VersionMismatchBanner({ slug, viewingVersion, mismatched }: { slug: string; viewingVersion: string; mismatched: RepoLibraryUsage[] }) {
  const router = useRouter();
  if (mismatched.length === 0) return null;

  // The prototype's banner has a single "View {declared}" action -- only
  // offer it when every mismatched repo actually declares the *same*
  // version; with a genuine mix of declared versions there's no single
  // target to jump to, so the button is omitted rather than guessing one.
  const declaredVersions = new Set(mismatched.map((u) => u.declared_version));
  const singleDeclaredVersion = declaredVersions.size === 1 ? mismatched[0].declared_version : null;

  return (
    <div className="flex flex-wrap items-center gap-3 border-b border-health-warning/40 bg-health-warning/[0.08] px-6 py-2.5 text-[14px]">
      <span className="size-2 shrink-0 rotate-45 bg-health-warning" />
      <span className="flex-1 text-ink-100">
        <span className="font-semibold text-health-warning">Version mismatch: </span>
        You are viewing docs for <span className="text-mono-code">{viewingVersion}</span>, but{" "}
        {mismatched.map((u, i) => (
          <span key={u.repo_identifier}>
            {i > 0 && ", "}
            <span className="text-mono-code text-ink-200">{u.repo_identifier}</span> has <span className="text-mono-code">{u.declared_version}</span> declared
          </span>
        ))}
        .
      </span>
      {singleDeclaredVersion && (
        <button
          type="button"
          onClick={() => router.push(`/libraries/${encodeURIComponent(slug)}?version=${encodeURIComponent(singleDeclaredVersion)}`)}
          className="shrink-0 rounded-full border border-health-warning px-3 py-1 text-[13px] font-semibold text-health-warning transition-colors hover:bg-health-warning/10"
        >
          View {singleDeclaredVersion}
        </button>
      )}
    </div>
  );
}
