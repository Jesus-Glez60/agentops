// No settings/preferences backend exists anywhere in this product yet (no
// per-user config, no auth, nothing to configure server-side beyond env
// vars). Rendered as an honest "not built yet" page -- kept in the nav
// since the mockup includes it -- rather than a fake settings form with
// nothing real behind it.
export default function SettingsPage() {
  return (
    <div className="flex flex-col items-start gap-2">
      <h1 className="text-page-title font-bold">Settings</h1>
      <p className="max-w-prose text-body text-ink-500">
        There&apos;s no per-user or per-deployment settings surface yet — configuration today is entirely
        environment variables on the backend services. This page is a placeholder for when that changes.
      </p>
    </div>
  );
}
