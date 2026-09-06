// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import mdx from '@astrojs/mdx';
import { SITE, BASE } from './src/site.config.mjs';
import { satteri } from '@astrojs/markdown-satteri';
import hastBaseLinks from './src/plugins/hast-base-links.mjs';

// Where the site is served from lives in src/site.config.mjs. Internal links in
// content are written without the base and get it added at build time, so the
// same content works under /openHC/ on project Pages or at / on a custom domain.
export default defineConfig({
  site: SITE,
  base: BASE || '/',
  trailingSlash: 'always',
  markdown: { processor: satteri({ hastPlugins: [hastBaseLinks] }) },
  integrations: [
    starlight({
      title: 'openHC',
      description:
        'Open, kernel-up firmware for Control4 controllers — and the hardware research behind it.',
      logo: { src: './src/assets/logo.svg', replacesTitle: false },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/The1TrueJoe/openHC' },
      ],
      editLink: {
        baseUrl: 'https://github.com/The1TrueJoe/openHC/edit/main/site/',
      },
      customCss: ['./src/styles/theme.css'],
      lastUpdated: true,
      pagination: true,
      credits: false,
      tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
      components: {
        // Adds the "Chapter N" eyebrow + reading estimate to build-log pages.
        PageTitle: './src/components/PageTitle.astro',
        // Dev: open the source file in VS Code. Production: the GitHub edit link.
        EditLink: './src/components/EditLink.astro',
      },
      sidebar: [
        {
          label: 'Start here',
          items: [
            { slug: 'index', label: 'What openHC is' },
            { slug: 'start/status', label: 'Board status' },
            { slug: 'start/glossary', label: 'Glossary' },
          ],
        },
        {
          label: 'The build log',
          collapsed: false,
          items: [{ autogenerate: { directory: 'log' } }],
        },
        {
          label: 'Hardware — the line',
          items: [
            { slug: 'hardware/matrix', label: 'The controller matrix' },
            { slug: 'hardware/how-they-differ', label: 'Four machines, four ways in' },
          ],
        },
        {
          label: 'EA family (Intel CE5310)',
          collapsed: false,
          items: [{ autogenerate: { directory: 'ea' } }],
        },
        {
          label: 'HC-800 (Atom D525)',
          collapsed: true,
          items: [{ autogenerate: { directory: 'hc800' } }],
        },
        {
          label: 'CA-1 (i.MX6 SoloLite)',
          collapsed: true,
          items: [{ autogenerate: { directory: 'ca1' } }],
        },
        {
          label: 'IO Extender V1 (TI DM355)',
          collapsed: true,
          items: [{ autogenerate: { directory: 'iox' } }],
        },
        {
          label: 'Shared subsystems',
          collapsed: true,
          items: [{ autogenerate: { directory: 'shared' } }],
        },
        {
          label: 'Build & install',
          collapsed: true,
          items: [{ autogenerate: { directory: 'build' } }],
        },
      ],
    }),
    // starlight() registers astro-expressive-code, which must be set up
    // before mdx() or code blocks in .mdx pages lose their highlighting.
    mdx(),
  ],
});
