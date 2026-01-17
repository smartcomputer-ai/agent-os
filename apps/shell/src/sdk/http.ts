import type { paths } from "./types";

const DEFAULT_API_BASE = "http://localhost:7777";
const API_BASE = import.meta.env.VITE_API_BASE || DEFAULT_API_BASE;

type HttpMethod = "get" | "post" | "put" | "delete" | "patch";

type PathsWithMethod<M extends HttpMethod> = {
  [P in keyof paths]: paths[P] extends { [K in M]?: unknown } ? P : never;
}[keyof paths];

type Operation<P extends keyof paths, M extends HttpMethod> = NonNullable<
  paths[P][M]
>;

type OperationParameters<Op> = Op extends { parameters: infer P } ? P : never;

type PathParams<Op> = OperationParameters<Op> extends { path?: infer P }
  ? P
  : never;

type QueryParams<Op> = OperationParameters<Op> extends { query?: infer Q }
  ? Q
  : never;

type JsonRequestBody<Op> = Op extends {
  requestBody: { content: { "application/json": infer T } };
}
  ? T
  : never;

type SuccessResponse<Op> = Op extends { responses: infer R }
  ? {
      [K in keyof R]: K extends 200 | 201 | 202 | 204 ? R[K] : never;
    }[keyof R]
  : never;

type JsonResponse<Op> = SuccessResponse<Op> extends {
  content: { "application/json": infer T };
}
  ? T
  : never;

type RequestOptions<Op> = {
  pathParams?: PathParams<Op>;
  query?: QueryParams<Op>;
  body?: JsonRequestBody<Op>;
  headers?: HeadersInit;
  signal?: AbortSignal;
};

export class ApiError extends Error {
  status: number;
  body: unknown;

  constructor(status: number, body: unknown) {
    super(`API request failed with status ${status}`);
    this.status = status;
    this.body = body;
  }
}

function applyPathParams(
  path: string,
  params: Record<string, string | number | boolean> | undefined,
): string {
  if (!params) {
    return path;
  }

  return path.replace(/\{([^}]+)\}/g, (...args) => {
    const key = args[1];
    if (!(key in params)) {
      throw new Error(`Missing path param: ${key}`);
    }
    return encodeURIComponent(String(params[key]));
  });
}

function appendQueryParams(
  url: URL,
  query: Record<string, unknown> | undefined,
): void {
  if (!query) {
    return;
  }

  for (const [key, raw] of Object.entries(query)) {
    if (raw === undefined || raw === null) {
      continue;
    }

    if (Array.isArray(raw)) {
      for (const item of raw) {
        if (item === undefined || item === null) {
          continue;
        }
        url.searchParams.append(key, String(item));
      }
    } else {
      url.searchParams.set(key, String(raw));
    }
  }
}

function buildUrl(
  path: string,
  options: { pathParams?: Record<string, string | number | boolean>; query?: Record<string, unknown> },
): string {
  const resolvedPath = applyPathParams(path, options.pathParams);
  const url = new URL(resolvedPath, API_BASE);
  appendQueryParams(url, options.query);
  return url.toString();
}

async function parseJsonOrText(response: Response): Promise<unknown> {
  const contentType = response.headers.get("content-type") ?? "";
  if (contentType.includes("application/json")) {
    return response.json();
  }
  return response.text();
}

export async function apiRequestJson<
  M extends HttpMethod,
  P extends PathsWithMethod<M>,
>(
  method: M,
  path: P,
  options: RequestOptions<Operation<P, M>> = {},
): Promise<JsonResponse<Operation<P, M>>> {
  const url = buildUrl(path, {
    pathParams: options.pathParams as Record<string, string | number | boolean> | undefined,
    query: options.query as Record<string, unknown> | undefined,
  });

  const headers = new Headers(options.headers);
  if (!headers.has("Accept")) {
    headers.set("Accept", "application/json");
  }

  const hasBody = options.body !== undefined;
  if (hasBody && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }

  const response = await fetch(url, {
    method: method.toUpperCase(),
    headers,
    body: hasBody ? JSON.stringify(options.body) : undefined,
    signal: options.signal,
  });

  if (!response.ok) {
    const body = await parseJsonOrText(response);
    throw new ApiError(response.status, body);
  }

  if (response.status === 204) {
    return undefined as JsonResponse<Operation<P, M>>;
  }

  const json = await response.json();
  return json as JsonResponse<Operation<P, M>>;
}

export async function apiRequestBinary<
  M extends HttpMethod,
  P extends PathsWithMethod<M>,
>(
  method: M,
  path: P,
  options: RequestOptions<Operation<P, M>> = {},
): Promise<ArrayBuffer> {
  const url = buildUrl(path, {
    pathParams: options.pathParams as Record<string, string | number | boolean> | undefined,
    query: options.query as Record<string, unknown> | undefined,
  });

  const headers = new Headers(options.headers);
  if (!headers.has("Accept")) {
    headers.set("Accept", "application/octet-stream");
  }

  const hasBody = options.body !== undefined;
  if (hasBody && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }

  const response = await fetch(url, {
    method: method.toUpperCase(),
    headers,
    body: hasBody ? JSON.stringify(options.body) : undefined,
    signal: options.signal,
  });

  if (!response.ok) {
    const body = await parseJsonOrText(response);
    throw new ApiError(response.status, body);
  }

  if (response.status === 204) {
    return new ArrayBuffer(0);
  }

  return response.arrayBuffer();
}
