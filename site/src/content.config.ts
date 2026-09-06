import { defineCollection } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { z } from 'astro:content';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    // `topic` drives the build-log eyebrow, `summary` the standfirst.
    // Deliberately no `date`: these entries are ordered by what was learned,
    // not by when it happened to be committed.
    schema: docsSchema({
      extend: z.object({
        topic: z.string().optional(),
        summary: z.string().optional(),
      }),
    }),
  }),
};
