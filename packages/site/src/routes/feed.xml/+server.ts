import { buildFeedXml, feedContentType } from '$lib/feed';

export const prerender = true;

export function GET(): Response {
  return new Response(buildFeedXml(), {
    headers: {
      'content-type': feedContentType
    }
  });
}
