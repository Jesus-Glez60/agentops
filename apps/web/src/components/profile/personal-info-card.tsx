"use client";

import { useState } from "react";
import { Pencil, X } from "lucide-react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import type { SessionUser } from "@/lib/auth/types";
import { PROFILE_SWR_KEY, updateProfile } from "@/lib/api/profile-api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

export function PersonalInfoCard({ user }: { user: SessionUser }) {
  const { mutate } = useSWRConfig();
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [form, setForm] = useState({ first_name: user.first_name, last_name: user.last_name, handle: user.handle ?? "", bio: user.bio, location: user.location });

  function startEdit() {
    setForm({ first_name: user.first_name, last_name: user.last_name, handle: user.handle ?? "", bio: user.bio, location: user.location });
    setEditing(true);
  }

  async function save() {
    setSaving(true);
    try {
      await updateProfile(form);
      await mutate(PROFILE_SWR_KEY);
      toast.success("Profile updated");
      setEditing(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update your profile. Please try again.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between border-b border-border-strong pb-4">
        <CardTitle>Personal Information</CardTitle>
        <Button variant="outline" size="sm" onClick={() => (editing ? setEditing(false) : startEdit())}>
          {editing ? (
            <>
              <X className="size-3.5" /> Cancel
            </>
          ) : (
            <>
              <Pencil className="size-3.5" /> Edit
            </>
          )}
        </Button>
      </CardHeader>
      <CardContent>
        {editing ? (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="Full name">
              <div className="flex gap-2">
                <Input value={form.first_name} onChange={(e) => setForm((f) => ({ ...f, first_name: e.target.value }))} placeholder="First name" />
                <Input value={form.last_name} onChange={(e) => setForm((f) => ({ ...f, last_name: e.target.value }))} placeholder="Last name" />
              </div>
            </Field>
            <Field label="Handle">
              <Input value={form.handle} onChange={(e) => setForm((f) => ({ ...f, handle: e.target.value }))} placeholder="handle" />
            </Field>
            <Field label="Location">
              <Input value={form.location} onChange={(e) => setForm((f) => ({ ...f, location: e.target.value }))} placeholder="San Francisco, CA" />
            </Field>
            <Field label="Bio" className="sm:col-span-2">
              <Textarea rows={3} value={form.bio} onChange={(e) => setForm((f) => ({ ...f, bio: e.target.value }))} />
            </Field>
            <div className="flex items-center gap-2 sm:col-span-2">
              <Button size="sm" disabled={saving} onClick={save}>
                {saving ? "Saving…" : "Save changes"}
              </Button>
              <Button size="sm" variant="outline" disabled={saving} onClick={() => setEditing(false)}>
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="Full name">{user.first_name} {user.last_name}</Field>
            <Field label="Handle">{user.handle ? `@${user.handle}` : <span className="text-ink-500">Not set</span>}</Field>
            <Field label="Email address">{user.email}</Field>
            <Field label="Location">{user.location || <span className="text-ink-500">Not set</span>}</Field>
            <Field label="Bio" className="sm:col-span-2">
              {user.bio || <span className="text-ink-500">No bio yet.</span>}
            </Field>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function Field({ label, children, className }: { label: string; children: React.ReactNode; className?: string }) {
  return (
    <div className={className}>
      <p className="mb-1 text-mono-code uppercase text-ink-500">{label}</p>
      <div className="text-body text-ink-100">{children}</div>
    </div>
  );
}
