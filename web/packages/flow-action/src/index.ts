/**
 * @homeroute/flow-action — handler factory for hr-flowd callbacks.
 *
 * Mount on a Next.js (or any Node web framework) catchall route:
 *
 * ```ts
 * // app/api/_flow/[type]/[name]/route.ts
 * import { handleFlowCallback } from '@homeroute/flow-action';
 * import { computeScore, enrichProfile } from '@/lib/flow-actions';
 *
 * export const runtime = 'nodejs'; // required: timingSafeEqual is not available in edge runtime
 * export const POST = handleFlowCallback({
 *   actions: { compute_score: computeScore, enrich_profile: enrichProfile },
 *   connectors: {
 *     // optional custom connectors:
 *     // openrouter: { chat: async (params, ctx) => { ... } },
 *   },
 * });
 * ```
 *
 * Bearer auth uses constant-time comparison; panics in user code are caught
 * and turned into structured `{ error, kind: "panic" }` responses, never raw
 * 500s. The daemon synthesises the same error shape for transport-level
 * failures (timeout, 5xx, unreachable) so apps don't need to handle that.
 */

import { timingSafeEqual } from 'node:crypto';

export interface FlowCallbackContext {
  runId: string;
  stepId: string;
  deadlineMs: number;
}

export type FlowAction = (
  input: unknown,
  params: unknown,
  ctx: FlowCallbackContext,
) => Promise<unknown> | unknown;

export type FlowConnectorOps = Record<string, FlowAction>;

export interface HandleFlowCallbackOptions {
  /** Map of action name → handler. */
  actions?: Record<string, FlowAction>;
  /** Map of connector name → ops map. */
  connectors?: Record<string, FlowConnectorOps>;
  /** Bearer token. Defaults to `process.env.HR_FLOW_TOKEN`. */
  token?: string;
}

interface CallbackBody {
  run_id?: string;
  step_id?: string;
  input?: unknown;
  params?: unknown;
}

/** Build a Next.js / web-fetch compatible POST handler. */
export function handleFlowCallback(opts: HandleFlowCallbackOptions) {
  const expected = opts.token ?? process.env.HR_FLOW_TOKEN;
  if (!expected || expected.length < 16) {
    throw new Error(
      '@homeroute/flow-action: HR_FLOW_TOKEN env var must be set (≥16 chars) or pass `token` explicitly',
    );
  }

  const expectedBytes = Buffer.from(expected, 'utf8');

  const actions = opts.actions ?? {};
  const connectors = opts.connectors ?? {};

  return async (req: Request): Promise<Response> => {
    // Bearer auth
    const auth = req.headers.get('authorization') ?? '';
    const presented = auth.startsWith('Bearer ') ? auth.slice('Bearer '.length) : '';
    const presentedBytes = Buffer.from(presented, 'utf8');
    if (
      presentedBytes.length !== expectedBytes.length ||
      !timingSafeEqual(presentedBytes, expectedBytes)
    ) {
      return new Response(null, { status: 401 });
    }

    // Path matching: /_flow/action/{name} or /_flow/connector/{name}/{op}
    const url = new URL(req.url);
    const parts = url.pathname.split('/').filter(Boolean);
    // parts is e.g. ['api', '_flow', 'action', 'compute_score']
    // or ['api', '_flow', 'connector', 'openrouter', 'chat']
    const flowIdx = parts.indexOf('_flow');
    if (flowIdx === -1 || flowIdx + 1 >= parts.length) {
      return errorJson('bad_path', `unrecognised callback path: ${url.pathname}`);
    }
    const kind = parts[flowIdx + 1];
    const name = parts[flowIdx + 2];

    let body: CallbackBody;
    try {
      body = (await req.json()) as CallbackBody;
    } catch (e) {
      return errorJson('bad_request', `body is not valid JSON: ${e instanceof Error ? e.message : String(e)}`);
    }
    const ctx: FlowCallbackContext = {
      runId: body.run_id ?? '',
      stepId: body.step_id ?? '',
      deadlineMs: parseInt(req.headers.get('x-flow-deadline-ms') ?? '30000', 10) || 30000,
    };

    if (kind === 'action') {
      if (!name) return errorJson('bad_path', 'missing action name');
      const handler = actions[name];
      if (!handler) return errorJson('unknown_action', `action "${name}" is not registered`);
      return invoke(() => handler(body.input, body.params, ctx), name, 'action');
    }

    if (kind === 'connector') {
      const op = parts[flowIdx + 3];
      if (!name || !op) return errorJson('bad_path', 'missing connector or op name');
      const connector = connectors[name];
      if (!connector) return errorJson('unknown_connector', `connector "${name}" is not registered`);
      const handler = connector[op];
      if (!handler) {
        return errorJson('unknown_op', `connector "${name}" has no op "${op}"`);
      }
      return invoke(() => handler(body.input, body.params, ctx), `${name}.${op}`, 'connector');
    }

    return errorJson('bad_path', `unknown kind "${kind}" (expected "action" or "connector")`);
  };
}

async function invoke(
  fn: () => Promise<unknown> | unknown,
  label: string,
  kind: 'action' | 'connector',
): Promise<Response> {
  try {
    const output = await fn();
    return jsonResponse({ output: output ?? null });
  } catch (err) {
    if (err instanceof Error) {
      // eslint-disable-next-line no-console
      console.warn(`flow-action: ${kind} "${label}" failed:`, err.message);
      return jsonResponse({
        error: err.message,
        kind: 'error',
      });
    }
    return jsonResponse({
      error: String(err),
      kind: 'error',
    });
  }
}

function errorJson(kind: string, message: string): Response {
  return jsonResponse({ error: message, kind });
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
