import type { APIRoute } from 'astro';
import { description, groups, repo } from '../features';

// A plain summary for AI assistants and answer engines (llmstxt.org).
export const GET: APIRoute = ({ site }) => {
  const home = new URL(import.meta.env.BASE_URL.replace(/\/?$/, '/'), site);
  const sections = groups
    .map((g) => `## ${g.name}\n\n${g.items.map((f) => `- ${f.title}: ${f.text}`).join('\n')}`)
    .join('\n\n');
  const text = `# Tonic Player

> ${description}

${sections}

## Links

- [Website](${home})
- [Download](${repo}/releases/latest)
- [Source code](${repo}) (GPL-3.0-or-later)
`;
  return new Response(text, { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
};
