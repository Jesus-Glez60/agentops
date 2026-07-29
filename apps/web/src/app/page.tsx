import Link from "next/link";

const screens = [
  { href: "/repos", title: "Repo overview", desc: "Connected repos, last-scanned time, health status." },
  { href: "/graph", title: "Neuron / graph browser", desc: "Symbols, dependencies, and gotcha nodes wired into the code graph." },
  { href: "/docs", title: "Onboarding docs viewer", desc: "Rendered agentops-docgen output." },
  { href: "/libraries", title: "Docbrain library browser", desc: "Ingested libraries, versions, and visibility — live from docbrain-api." },
];

export default function Home() {
  return (
    <div className="flex flex-col flex-1 items-center bg-zinc-50 font-sans dark:bg-black">
      <main className="flex w-full max-w-3xl flex-col gap-8 py-24 px-8">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight text-black dark:text-zinc-50">
            agentops dashboard
          </h1>
          <p className="mt-2 text-zinc-600 dark:text-zinc-400">
            The neuron graph, onboarding docs, and library index — live from agentops-api and docbrain-api.
          </p>
        </div>
        <nav className="flex flex-col gap-4">
          {screens.map((s) => (
            <Link
              key={s.href}
              href={s.href}
              className="rounded-lg border border-black/[.08] p-5 transition-colors hover:bg-black/[.03] dark:border-white/[.145] dark:hover:bg-white/[.03]"
            >
              <div className="font-medium text-black dark:text-zinc-50">{s.title}</div>
              <div className="text-sm text-zinc-600 dark:text-zinc-400">{s.desc}</div>
            </Link>
          ))}
        </nav>
      </main>
    </div>
  );
}
