// @ts-check
import { defineConfig } from 'astro/config';

import react from '@astrojs/react';

// https://astro.build/config
export default defineConfig({
  // Powers canonical/Open Graph URLs in src/layouts/Layout.astro via
  // Astro.site -- the production domain, even though this same build also
  // gets deployed for local dev/preview under a different origin.
  site: 'https://agentops.dedyn.io',
  integrations: [react()]
});