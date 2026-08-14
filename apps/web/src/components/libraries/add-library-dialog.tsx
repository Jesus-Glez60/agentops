"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { Plus } from "lucide-react";
import { discoverLibrary, registerLibraryManually, scrapeLibrary, ECOSYSTEM_LABELS, LIBRARIES_SWR_KEY, type Ecosystem } from "@/lib/api/libraries-api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** "custom" isn't a real `Ecosystem` (no registry to query) -- it swaps the
 * form to caller-supplied docs URL + optional GitHub repo and calls
 * `register_library` directly instead of `discover_library`. */
type EcosystemOrCustom = Ecosystem | "custom";

const ECOSYSTEM_OPTIONS: EcosystemOrCustom[] = ["npm", "pypi", "cargo", "go", "nuget", "custom"];

/** No dedicated create-library REST route exists -- chains `/tools/*` calls: `discover_library` or `register_library` (registry metadata vs. caller-supplied, depending on ecosystem), then `scrape_library` (the actual doc ingestion). A registered-but-never-scraped library doesn't show up in the list at all (see docbrain-api's `list_libraries_json` filter), so this dialog has to do both steps or "Add library" would silently do nothing visible. */
export function AddLibraryDialog() {
  const { mutate } = useSWRConfig();
  const [open, setOpen] = useState(false);
  const [ecosystem, setEcosystem] = useState<EcosystemOrCustom>("npm");
  const [name, setName] = useState("");
  const [docsUrl, setDocsUrl] = useState("");
  const [githubRepo, setGithubRepo] = useState("");
  const [version, setVersion] = useState("");
  const [step, setStep] = useState<"idle" | "registering" | "scraping">("idle");

  const isCustom = ecosystem === "custom";
  const canSubmit = name.trim() && version.trim() && (!isCustom || docsUrl.trim());

  function reset() {
    setName("");
    setDocsUrl("");
    setGithubRepo("");
    setVersion("");
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmedName = name.trim();
    const trimmedVersion = version.trim();
    if (!trimmedName || !trimmedVersion || (isCustom && !docsUrl.trim())) return;

    setStep("registering");
    try {
      if (isCustom) {
        const registered = await registerLibraryManually(trimmedName, docsUrl.trim(), githubRepo.trim());
        if (registered.isError) {
          toast.error(registered.text || `Could not register '${trimmedName}'.`);
          return;
        }
      } else {
        const discovered = await discoverLibrary(ecosystem, trimmedName);
        // `discover_library` only sets `isError` for malformed args (bad
        // ecosystem, etc.) -- "not found on the registry" is a normal `Ok`
        // result with informational text, so check the text itself.
        // Nothing gets registered in that case, so scraping next would
        // just fail with "no library registered" -- stop here instead.
        if (discovered.isError || discovered.text.includes("not found")) {
          toast.error(`${discovered.text} Try "Custom / other" if it has its own docs site outside ${ECOSYSTEM_LABELS[ecosystem]}.`);
          return;
        }
      }

      setStep("scraping");
      const scraped = await scrapeLibrary(trimmedName, trimmedVersion, false);
      if (scraped.isError) {
        toast.error(scraped.text || `Registered '${trimmedName}' but scraping ${trimmedVersion} failed.`);
        return;
      }

      toast.success(scraped.text);
      await mutate(LIBRARIES_SWR_KEY);
      setOpen(false);
      reset();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Ingestion failed. Please try again.");
    } finally {
      setStep("idle");
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-3.5" />
          Add library
        </Button>
      </DialogTrigger>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add library</DialogTitle>
            <DialogDescription>
              {isCustom
                ? "For a library with no package-manager entry -- provide its docs site directly, then scrapes it for the version you specify."
                : "Looks up the package's registry metadata, then scrapes its docs for the version you specify -- this can take a little while for a large site."}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-4">
            <div className="flex gap-2">
              <Select value={ecosystem} onValueChange={(v) => setEcosystem(v as EcosystemOrCustom)}>
                <SelectTrigger className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {ECOSYSTEM_OPTIONS.map((eco) => (
                    <SelectItem key={eco} value={eco}>
                      {eco === "custom" ? "Custom / other" : ECOSYSTEM_LABELS[eco]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={isCustom ? "name, e.g. clerk" : "package name, e.g. next"} autoFocus />
            </div>
            {isCustom && (
              <>
                <Input value={docsUrl} onChange={(e) => setDocsUrl(e.target.value)} placeholder="docs URL, e.g. https://clerk.com/docs" />
                <Input value={githubRepo} onChange={(e) => setGithubRepo(e.target.value)} placeholder="GitHub repo (optional)" />
              </>
            )}
            <Input value={version} onChange={(e) => setVersion(e.target.value)} placeholder="version to scrape, e.g. 16 or 19.1.0" />
          </div>
          <DialogFooter>
            <Button type="submit" disabled={step !== "idle" || !canSubmit}>
              {step === "registering" ? "Registering…" : step === "scraping" ? "Scraping docs…" : "Discover & ingest"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
