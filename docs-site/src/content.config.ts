import { defineCollection, z } from "astro:content";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";
import { glob } from "astro/loaders";

// PROTOTYPE (see docs-site/src/content/docs/reference/examples/batch.mdx):
// alef-generated fixture snippets under src/snippets-generated/<language>/<category>/<topic>.md.
// The glob loader re-scans this tree on every build/dev restart, so alef adding or removing a
// fixture requires no manual index maintenance here. Frontmatter is validated against the schema
// below, so a malformed generated file fails the Astro build with a clear file:line error instead
// of a silent runtime gap.
const apiExamplesSchema = z.object({
  id: z.string(),
  language: z.string(),
  target: z.string(),
  level: z.string(),
  requires: z.array(z.string()),
  side_effect: z.string(),
});

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
  apiExamples: defineCollection({
    loader: glob({ pattern: "**/*.md", base: "./src/snippets-generated" }),
    schema: apiExamplesSchema,
  }),
};
