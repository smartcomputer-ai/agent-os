import { decode as cborDecode, encode as cborEncode, rfc8949EncodeOptions } from "cborg";

const SELF_DESCRIBE_TAG = new Uint8Array([0xd9, 0xd9, 0xf7]);
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true });

export function decodeCborFromBase64<T>(b64: string): T {
  const bytes = base64ToBytes(b64);
  const decoded = cborDecode(stripSelfDescribe(bytes));
  return decoded as T;
}

export function decodeCborTextFromBase64(b64: string): string {
  const decoded = decodeCborFromBase64<unknown>(b64);
  if (typeof decoded !== "string") {
    throw new Error("Expected CBOR text value");
  }
  return decoded;
}

export function displayKeyFromBase64(b64: string): string {
  const bytes = base64ToBytes(b64);
  try {
    const decoded = cborDecode(stripSelfDescribe(bytes));
    if (typeof decoded === "string") {
      return decoded;
    }
  } catch {
    // Fall through to UTF-8/hex fallback.
  }
  try {
    return UTF8_DECODER.decode(bytes);
  } catch {
    return `0x${bytesToHex(bytes)}`;
  }
}

export function encodeCborToBase64(value: unknown): string {
  return bytesToBase64(encodeCanonicalCbor(value));
}

export function encodeCborTextToBase64(text: string): string {
  return encodeCborToBase64(text);
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

function encodeCanonicalCbor(value: unknown): Uint8Array {
  const encoded = cborEncode(value, rfc8949EncodeOptions);
  const withTag = new Uint8Array(SELF_DESCRIBE_TAG.length + encoded.length);
  withTag.set(SELF_DESCRIBE_TAG, 0);
  withTag.set(encoded, SELF_DESCRIBE_TAG.length);
  return withTag;
}

function stripSelfDescribe(bytes: Uint8Array): Uint8Array {
  if (
    bytes.length >= SELF_DESCRIBE_TAG.length &&
    bytes[0] === SELF_DESCRIBE_TAG[0] &&
    bytes[1] === SELF_DESCRIBE_TAG[1] &&
    bytes[2] === SELF_DESCRIBE_TAG[2]
  ) {
    return bytes.subarray(SELF_DESCRIBE_TAG.length);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  let hex = "";
  for (const byte of bytes) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return hex;
}
