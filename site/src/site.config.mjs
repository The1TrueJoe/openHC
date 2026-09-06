// One place that knows where the site is served from.
//
// GitHub Pages serves a *project* site under /<repo>/, so the default base is
// '/openHC' and the published URL is https://the1truejoe.github.io/openHC/.
// Point a custom domain at it (or move it to a <user>.github.io repo) and it
// lives at the root instead — set OHC_SITE_BASE=/ and nothing else changes,
// because internal links are written base-agnostically and get the prefix at
// build time (see plugins/rehype-base-links.mjs).
export const SITE = process.env.OHC_SITE_URL ?? 'https://the1truejoe.github.io';

const rawBase = process.env.OHC_SITE_BASE ?? '/openHC';
// Normalise to a leading slash and no trailing slash ('/' stays '').
export const BASE = rawBase === '/' ? '' : '/' + rawBase.replace(/^\/|\/$/g, '');
