import { BASE } from '../site.config.mjs';

// Internal links in content are written WITHOUT the base — `/ea/ea1/`, not
// `/openHC/ea/ea1/` — so the same markdown works whether the site is served
// from a project-Pages subpath or from the root of a custom domain. This adds
// the prefix at build time.
//
// Written against Sätteri's hast visitor API (Astro 7's default Markdown
// processor) rather than as a rehype plugin, so the site keeps the default
// pipeline instead of pulling in the legacy unified processor.

const needsPrefix = (v) =>
  typeof v === 'string' &&
  v.startsWith('/') &&
  !v.startsWith('//') &&          // protocol-relative, external
  v !== BASE &&
  !v.startsWith(BASE + '/');      // already prefixed

export default function hastBaseLinks() {
  if (!BASE) return null;         // served at the root: nothing to do
  return {
    name: 'ohc-base-links',
    element: [
      {
        filter: ['a', 'img'],
        visit(node, ctx) {
          const attr = node.tagName === 'a' ? 'href' : 'src';
          const v = node.properties?.[attr];
          if (needsPrefix(v)) ctx.setProperty(node, attr, BASE + v);
        },
      },
    ],
    // Raw HTML blocks (the build-log card list) never become elements, so their
    // hrefs have to be rewritten in the source string.
    raw(node, ctx) {
      const html = node.value;
      if (typeof html !== 'string' || !html.includes('="/')) return;
      const out = html.replace(
        /\b(href|src)="(\/[^"]*)"/g,
        (m, attr, v) => (needsPrefix(v) ? `${attr}="${BASE}${v}"` : m),
      );
      if (out !== html) ctx.setProperty(node, 'value', out);
    },
  };
}
