import type { APIRoute } from 'astro';

// One page is worth indexing; the screensaver is kept out.
export const GET: APIRoute = ({ site }) => {
  const home = new URL(import.meta.env.BASE_URL.replace(/\/?$/, '/'), site);
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>${home}</loc></url>
</urlset>
`;
  return new Response(xml, { headers: { 'Content-Type': 'application/xml' } });
};
